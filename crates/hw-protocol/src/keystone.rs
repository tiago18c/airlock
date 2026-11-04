//! Keystone: the UR types its Solana support reads and writes over QR, as
//! Keystone's `ur-registry` lays them out.
//!
//! - `sol-sign-request` (1101): what to sign, with whose key.
//! - `sol-signature` (1102): the signature, with the request's id.
//! - `crypto-multi-accounts` (1103): the keys a Keystone exports.

use crate::path::HARDENED;
use ur_codec::{CborError, Value};

pub const SOL_SIGN_REQUEST: &str = "sol-sign-request";
pub const SOL_SIGNATURE: &str = "sol-signature";
pub const CRYPTO_MULTI_ACCOUNTS: &str = "crypto-multi-accounts";

const TAG_UUID: u64 = 37;
const TAG_HDKEY: u64 = 303;
const TAG_KEYPATH: u64 = 304;

/// A path with the fingerprint of the seed it starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPath {
    pub indices: Vec<u32>,
    pub source_fingerprint: Option<u32>,
}

impl KeyPath {
    fn to_value(&self) -> Value {
        let components = self
            .indices
            .iter()
            .flat_map(|index| {
                [
                    Value::Uint(u64::from(index & !HARDENED)),
                    Value::Bool(index & HARDENED != 0),
                ]
            })
            .collect();
        let mut map = vec![(Value::Uint(1), Value::Array(components))];
        if let Some(fingerprint) = self.source_fingerprint {
            map.push((Value::Uint(2), Value::Uint(u64::from(fingerprint))));
        }
        Value::Tag(TAG_KEYPATH, Box::new(Value::Map(map)))
    }

    fn from_value(value: &Value) -> Result<Self, CborError> {
        let value = value.untag(TAG_KEYPATH)?;
        let components = value.need(1)?.as_array()?;
        if !components.len().is_multiple_of(2) {
            return Err(CborError::Shape("index and hardened pairs"));
        }
        let indices = components
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[index, hardened]| {
                let index = index.as_u32()?;
                if index >= HARDENED {
                    return Err(CborError::Shape("an index below 2^31"));
                }
                Ok(if hardened.as_bool()? {
                    index | HARDENED
                } else {
                    index
                })
            })
            .collect::<Result<_, _>>()?;
        let source_fingerprint = value.get(2)?.map(Value::as_u32).transpose()?;
        Ok(Self {
            indices,
            source_fingerprint,
        })
    }
}

/// What a `sol-sign-request` asks to sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignType {
    /// A transaction's message.
    Transaction = 1,
    /// Bytes signed as they are.
    Message = 2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolSignRequest {
    pub request_id: [u8; 16],
    pub sign_data: Vec<u8>,
    pub path: KeyPath,
    pub address: Option<Vec<u8>>,
    /// Shown on the device: who asks.
    pub origin: Option<String>,
    pub sign_type: SignType,
}

impl SolSignRequest {
    pub fn to_cbor(&self) -> Vec<u8> {
        let mut map = vec![
            (
                Value::Uint(1),
                Value::Tag(TAG_UUID, Box::new(Value::Bytes(self.request_id.to_vec()))),
            ),
            (Value::Uint(2), Value::Bytes(self.sign_data.clone())),
            (Value::Uint(3), self.path.to_value()),
        ];
        if let Some(address) = &self.address {
            map.push((Value::Uint(4), Value::Bytes(address.clone())));
        }
        if let Some(origin) = &self.origin {
            map.push((Value::Uint(5), Value::Text(origin.clone())));
        }
        map.push((Value::Uint(6), Value::Uint(self.sign_type as u64)));
        Value::Map(map).encode()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolSignature {
    pub request_id: Option<[u8; 16]>,
    pub signature: [u8; 64],
}

impl SolSignature {
    pub fn from_cbor(bytes: &[u8]) -> Result<Self, CborError> {
        let value = Value::decode(bytes)?;
        let request_id = value
            .get(1)?
            .map(|id| -> Result<[u8; 16], CborError> {
                id.untag(TAG_UUID)?
                    .as_bytes()?
                    .try_into()
                    .map_err(|_| CborError::Shape("a 16-byte request id"))
            })
            .transpose()?;
        let signature = value
            .need(2)?
            .as_bytes()?
            .try_into()
            .map_err(|_| CborError::Shape("a 64-byte signature"))?;
        Ok(Self {
            request_id,
            signature,
        })
    }

    #[cfg(any(test, feature = "fake"))]
    pub fn to_cbor(&self) -> Vec<u8> {
        let mut map = Vec::new();
        if let Some(id) = self.request_id {
            map.push((
                Value::Uint(1),
                Value::Tag(TAG_UUID, Box::new(Value::Bytes(id.to_vec()))),
            ));
        }
        map.push((Value::Uint(2), Value::Bytes(self.signature.to_vec())));
        Value::Map(map).encode()
    }
}

/// One key of an export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportedKey {
    /// The Ed25519 key, 32 bytes, for a Solana account.
    pub key: Vec<u8>,
    pub path: Option<KeyPath>,
    pub name: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiAccounts {
    pub master_fingerprint: u32,
    pub keys: Vec<ExportedKey>,
    pub device: Option<String>,
    pub device_id: Option<String>,
    pub firmware: Option<String>,
}

impl MultiAccounts {
    pub fn from_cbor(bytes: &[u8]) -> Result<Self, CborError> {
        let value = Value::decode(bytes)?;
        let keys = value
            .need(2)?
            .as_array()?
            .iter()
            .map(|key| {
                let key = key.untag(TAG_HDKEY)?;
                Ok(ExportedKey {
                    key: key.need(3)?.as_bytes()?.to_vec(),
                    path: key.get(6)?.map(KeyPath::from_value).transpose()?,
                    name: key
                        .get(9)?
                        .map(|v| v.as_text().map(str::to_string))
                        .transpose()?,
                    note: key
                        .get(10)?
                        .map(|v| v.as_text().map(str::to_string))
                        .transpose()?,
                })
            })
            .collect::<Result<_, CborError>>()?;
        let text = |key| -> Result<Option<String>, CborError> {
            value
                .get(key)?
                .map(|v| v.as_text().map(str::to_string))
                .transpose()
        };
        Ok(Self {
            master_fingerprint: value.need(1)?.as_u32()?,
            keys,
            device: text(3)?,
            device_id: text(4)?,
            firmware: text(5)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::parse_path;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect()
    }

    /// The request in `ur-registry`'s own test.
    #[test]
    fn a_sign_request_is_written_as_keystone_writes_it() {
        let sign_data = unhex("01000103c8d842a2f17fd7aab608ce2ea535a6e958dffa20caf669b347b911c4171965530f957620b228bae2b94c82ddd4c093983a67365555b737ec7ddc1117e61c72e0000000000000000000000000000000000000000000000000000000000000000010295cc2f1f39f3604718496ea00676d6a72ec66ad09d926e3ece34f565f18d201020200010c0200000000e1f50500000000");
        let request = SolSignRequest {
            request_id: [
                155, 29, 235, 77, 59, 125, 75, 173, 155, 221, 43, 13, 123, 61, 203, 109,
            ],
            sign_data,
            path: KeyPath {
                indices: parse_path("m/44'/501'/0'/0'").unwrap(),
                source_fingerprint: Some(0x1212_1212),
            },
            address: None,
            origin: Some("solflare".into()),
            sign_type: SignType::Transaction,
        };
        assert_eq!(hex(&request.to_cbor()), "a501d825509b1deb4d3b7d4bad9bdd2b0d7b3dcb6d02589601000103c8d842a2f17fd7aab608ce2ea535a6e958dffa20caf669b347b911c4171965530f957620b228bae2b94c82ddd4c093983a67365555b737ec7ddc1117e61c72e0000000000000000000000000000000000000000000000000000000000000000010295cc2f1f39f3604718496ea00676d6a72ec66ad09d926e3ece34f565f18d201020200010c0200000000e1f5050000000003d90130a20188182cf51901f5f500f500f5021a121212120568736f6c666c6172650601");
    }

    #[test]
    fn a_signature_is_read_as_keystone_writes_it() {
        let signature = SolSignature::from_cbor(&unhex("a201d825509b1deb4d3b7d4bad9bdd2b0d7b3dcb6d025840d4f0a7bcd95bba1fbb1051885054730e3f47064288575aacc102fbbf6a9a14daa066991e360d3e3406c20c00a40973eff37c7d641e5b351ec4a99bfe86f335f7")).unwrap();
        assert_eq!(signature.request_id.unwrap()[0], 155);
        assert_eq!(signature.signature[..2], [0xd4, 0xf0]);
        assert_eq!(SolSignature::from_cbor(&signature.to_cbor()), Ok(signature));
    }

    #[test]
    fn an_export_is_read_as_keystone_writes_it() {
        let accounts = MultiAccounts::from_cbor(&unhex("a5011ae9181cf30281d9012fa203582102eae4b876a8696134b868f88cc2f51f715f2dbedb7446b8e6edf3d4541c4eb67b06d90130a10188182cf51901f5f500f500f503686b657973746f6e65047828323834373563386438306636633036626166626534366137643137353066336663663235363566370565312e302e30")).unwrap();
        assert_eq!(accounts.master_fingerprint, 0xe918_1cf3);
        assert_eq!(accounts.device.as_deref(), Some("keystone"));
        assert_eq!(accounts.firmware.as_deref(), Some("1.0.0"));
        assert_eq!(accounts.keys.len(), 1);
        assert_eq!(
            accounts.keys[0].path.as_ref().unwrap().indices,
            parse_path("m/44'/501'/0'/0'").unwrap()
        );
    }
}
