//! A payload as JSON: the text a person copies, reads and pastes.
//!
//! One object is a JSON object, several are an array of them, each tagged by
//! `"type"`. Addresses and Ed25519 signatures are base58, as Solana prints
//! them. SHA-256 digests and nonces are hex. Bytes of any other length (a
//! wire, a key, a signature of another scheme, account data) are base64.
//!
//! ```json
//! {"type":"signature","signs":"transaction","pubkey":"<base58>","signature":"<base58>"}
//! ```
//!
//! The wire form ([`encode_payload`]) carries the same objects. These types
//! only translate.

use super::object::check_payload;
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Payload {
    One(Object),
    Many(Vec<Object>),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Object {
    Transaction {
        #[serde(with = "b64")]
        wire: Vec<u8>,
    },
    Signature(Signature),
    #[serde(rename_all = "camelCase")]
    ContextNote {
        #[serde(with = "hex32")]
        message: [u8; 32],
        #[serde(default)]
        pins: Vec<Pin>,
        #[serde(default)]
        authorizations: Vec<Authorization>,
        #[serde(default)]
        accounts: Vec<Account>,
    },
    PublicKey {
        scheme: u8,
        #[serde(with = "b64")]
        key: Vec<u8>,
    },
    WalletKeys {
        keys: Vec<Key>,
    },
    #[serde(rename_all = "camelCase")]
    KeyRotation {
        #[serde(with = "b58")]
        account: [u8; PUBKEY_BYTES],
        scheme: u8,
        #[serde(with = "b64")]
        next_key: Vec<u8>,
        signed: Vec<NonceSignature>,
    },
    #[serde(rename_all = "camelCase")]
    AccountState {
        #[serde(with = "b58")]
        account: [u8; PUBKEY_BYTES],
        scheme: u8,
        #[serde(with = "hex32")]
        nonce: [u8; 32],
        leaf_floor: u32,
        #[serde(with = "b64")]
        key: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "signs", rename_all = "camelCase", deny_unknown_fields)]
enum Signature {
    Transaction {
        #[serde(with = "b58")]
        pubkey: [u8; PUBKEY_BYTES],
        #[serde(with = "b58")]
        signature: [u8; SIGNATURE_BYTES],
    },
    Authorization {
        #[serde(with = "b58")]
        authority: [u8; PUBKEY_BYTES],
        #[serde(with = "b58")]
        signature: [u8; SIGNATURE_BYTES],
    },
    Account {
        #[serde(with = "b58")]
        account: [u8; PUBKEY_BYTES],
        #[serde(with = "hex32")]
        nonce: [u8; 32],
        #[serde(with = "b64")]
        signature: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    #[serde(with = "b58")]
    program: [u8; PUBKEY_BYTES],
    #[serde(with = "hex32")]
    idl: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Authorization {
    #[serde(with = "b58")]
    account: [u8; PUBKEY_BYTES],
    scheme: u8,
    #[serde(with = "hex32")]
    nonce: [u8; 32],
    leaf_floor: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    #[serde(with = "b58")]
    pubkey: [u8; PUBKEY_BYTES],
    #[serde(with = "b64")]
    data: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NonceSignature {
    #[serde(with = "hex32")]
    nonce: [u8; 32],
    #[serde(with = "b64")]
    signature: Vec<u8>,
}

/// A wallet key: an address in base58, or a Vector account's key in base64
/// under its scheme.
#[derive(Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase", deny_unknown_fields)]
enum Key {
    Address {
        #[serde(default)]
        label: String,
        #[serde(with = "b58")]
        key: [u8; PUBKEY_BYTES],
    },
    Vector {
        #[serde(default)]
        label: String,
        scheme: u8,
        #[serde(with = "b64")]
        key: Vec<u8>,
    },
}

impl From<&QrObject> for Object {
    fn from(object: &QrObject) -> Self {
        match object.clone() {
            QrObject::Transaction(wire) => Object::Transaction { wire },
            QrObject::Signature(signed) => Object::Signature(match signed {
                Signed::Transaction { pubkey, signature } => {
                    Signature::Transaction { pubkey, signature }
                }
                Signed::Authorization {
                    authority,
                    signature,
                } => Signature::Authorization {
                    authority,
                    signature,
                },
                Signed::AccountAuthorization {
                    account,
                    nonce,
                    signature,
                } => Signature::Account {
                    account,
                    nonce,
                    signature,
                },
            }),
            QrObject::Context(context) => Object::ContextNote {
                message: context.message_sha256,
                pins: context
                    .pins
                    .into_iter()
                    .map(|pin| Pin {
                        program: pin.program_id,
                        idl: pin.idl_sha256,
                    })
                    .collect(),
                authorizations: context
                    .authorizations
                    .into_iter()
                    .map(|one| Authorization {
                        account: one.account,
                        scheme: one.scheme,
                        nonce: one.nonce,
                        leaf_floor: one.leaf_floor,
                    })
                    .collect(),
                accounts: context
                    .accounts
                    .into_iter()
                    .map(|account| Account {
                        pubkey: account.pubkey,
                        data: account.data,
                    })
                    .collect(),
            },
            QrObject::PublicKey { scheme, key } => Object::PublicKey { scheme, key },
            QrObject::WalletKeys(keys) => Object::WalletKeys {
                keys: keys
                    .into_iter()
                    .map(|key| match key.role {
                        KeyRole::Address => Key::Address {
                            label: key.label,
                            // Checked to be an address before it is written.
                            key: key.key.try_into().unwrap_or_default(),
                        },
                        KeyRole::Vector { scheme } => Key::Vector {
                            label: key.label,
                            scheme,
                            key: key.key,
                        },
                    })
                    .collect(),
            },
            QrObject::KeyRotation(rotation) => Object::KeyRotation {
                account: rotation.account,
                scheme: rotation.scheme,
                next_key: rotation.next_key,
                signed: rotation
                    .signed
                    .into_iter()
                    .map(|one| NonceSignature {
                        nonce: one.nonce,
                        signature: one.signature,
                    })
                    .collect(),
            },
            QrObject::AccountState(state) => Object::AccountState {
                account: state.account,
                scheme: state.scheme,
                nonce: state.nonce,
                leaf_floor: state.leaf_floor,
                key: state.key,
            },
        }
    }
}

impl From<Object> for QrObject {
    fn from(object: Object) -> Self {
        match object {
            Object::Transaction { wire } => QrObject::Transaction(wire),
            Object::Signature(signature) => QrObject::Signature(match signature {
                Signature::Transaction { pubkey, signature } => {
                    Signed::Transaction { pubkey, signature }
                }
                Signature::Authorization {
                    authority,
                    signature,
                } => Signed::Authorization {
                    authority,
                    signature,
                },
                Signature::Account {
                    account,
                    nonce,
                    signature,
                } => Signed::AccountAuthorization {
                    account,
                    nonce,
                    signature,
                },
            }),
            Object::ContextNote {
                message,
                pins,
                authorizations,
                accounts,
            } => QrObject::Context(TxContext {
                message_sha256: message,
                pins: pins
                    .into_iter()
                    .map(|pin| ContextPin {
                        program_id: pin.program,
                        idl_sha256: pin.idl,
                    })
                    .collect(),
                authorizations: authorizations
                    .into_iter()
                    .map(|one| ContextAuthorization {
                        account: one.account,
                        scheme: one.scheme,
                        nonce: one.nonce,
                        leaf_floor: one.leaf_floor,
                    })
                    .collect(),
                accounts: accounts
                    .into_iter()
                    .map(|account| ContextAccount {
                        pubkey: account.pubkey,
                        data: account.data,
                    })
                    .collect(),
            }),
            Object::PublicKey { scheme, key } => QrObject::PublicKey { scheme, key },
            Object::WalletKeys { keys } => QrObject::WalletKeys(
                keys.into_iter()
                    .map(|key| match key {
                        Key::Address { label, key } => WalletKey {
                            role: KeyRole::Address,
                            label,
                            key: key.to_vec(),
                        },
                        Key::Vector { label, scheme, key } => WalletKey {
                            role: KeyRole::Vector { scheme },
                            label,
                            key,
                        },
                    })
                    .collect(),
            ),
            Object::KeyRotation {
                account,
                scheme,
                next_key,
                signed,
            } => QrObject::KeyRotation(KeyRotation {
                account,
                scheme,
                next_key,
                signed: signed
                    .into_iter()
                    .map(|one| RotationSignature {
                        nonce: one.nonce,
                        signature: one.signature,
                    })
                    .collect(),
            }),
            Object::AccountState {
                account,
                scheme,
                nonce,
                leaf_floor,
                key,
            } => QrObject::AccountState(AccountState {
                account,
                scheme,
                nonce,
                leaf_floor,
                key,
            }),
        }
    }
}

/// `objects` as pretty JSON: an object for one, an array for several. The
/// same limits as [`encode_payload`] apply.
pub fn payload_json(objects: &[QrObject]) -> Result<String, ProtocolError> {
    check_payload(objects)?;
    let payload = match objects {
        [one] => Payload::One(one.into()),
        many => Payload::Many(many.iter().map(Object::from).collect()),
    };
    serde_json::to_string_pretty(&payload).map_err(|_| ProtocolError::BadObject)
}

/// The objects of JSON text. Anything [`payload_json`] would refuse to
/// write is refused.
pub fn parse_payload_json(text: &str) -> Result<Vec<QrObject>, ProtocolError> {
    let payload: Payload = serde_json::from_str(text).map_err(|_| ProtocolError::BadObject)?;
    let objects: Vec<QrObject> = match payload {
        Payload::One(one) => vec![one.into()],
        Payload::Many(many) => many.into_iter().map(QrObject::from).collect(),
    };
    check_payload(&objects)?;
    Ok(objects)
}

mod b58 {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer, const N: usize>(
        bytes: &[u8; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&bs58::encode(bytes).into_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        deserializer: D,
    ) -> Result<[u8; N], D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = bs58::decode(&text).into_vec().map_err(D::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| D::Error::custom(format!("expected {N} bytes in base58")))
    }
}

mod hex32 {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = hex::decode(&text).map_err(D::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| D::Error::custom("expected 32 bytes in hex"))
    }
}

mod b64 {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(D::Error::custom)
    }
}
