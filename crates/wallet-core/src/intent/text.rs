//! How addresses, signatures and bytes are written in an intent's JSON:
//! addresses and signatures as base58, bytes as base64.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use std::str::FromStr;

/// An address from its text, with an error that names the text.
pub(crate) fn parse_address(text: &str) -> Result<Pubkey, String> {
    Pubkey::from_str(text).map_err(|_| format!("{text} is not an address"))
}

pub(crate) mod address {
    use super::*;

    pub fn serialize<S: Serializer>(pubkey: &Pubkey, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&pubkey.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Pubkey, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_address(&text).map_err(serde::de::Error::custom)
    }
}

/// An address as a map key or a list item.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Address(#[serde(with = "address")] pub Pubkey);

pub(crate) mod addresses {
    use super::*;

    pub fn serialize<S: Serializer>(keys: &[Pubkey], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(keys.iter().map(|key| Address(*key)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Pubkey>, D::Error> {
        let keys = Vec::<Address>::deserialize(deserializer)?;
        Ok(keys.into_iter().map(|key| key.0).collect())
    }
}

/// Address to address, as `replaced` writes it.
pub(crate) mod address_pairs {
    use super::*;
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(
        pairs: &BTreeMap<Pubkey, Pubkey>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_map(
            pairs
                .iter()
                .map(|(old, new)| (Address(*old), Address(*new))),
        )
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<Pubkey, Pubkey>, D::Error> {
        let pairs = BTreeMap::<Address, Address>::deserialize(deserializer)?;
        Ok(pairs.into_iter().map(|(old, new)| (old.0, new.0)).collect())
    }
}

pub(crate) mod signature {
    use super::*;

    pub fn serialize<S: Serializer>(
        signature: &Signature,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&signature.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Signature, D::Error> {
        let text = String::deserialize(deserializer)?;
        Signature::from_str(&text)
            .map_err(|_| serde::de::Error::custom(format!("{text} is not a signature")))
    }
}

pub(crate) mod option_signature {
    use super::*;

    #[derive(Serialize, Deserialize)]
    #[serde(transparent)]
    struct Text(#[serde(with = "signature")] Signature);

    pub fn serialize<S: Serializer>(
        signature: &Option<Signature>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        signature.map(Text).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Signature>, D::Error> {
        Ok(Option::<Text>::deserialize(deserializer)?.map(|text| text.0))
    }
}

pub(crate) mod base64_bytes {
    use super::*;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD
            .decode(text)
            .map_err(|_| serde::de::Error::custom("bytes have to be written as base64"))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct Bytes(#[serde(with = "base64_bytes")] Vec<u8>);

pub(crate) mod base64_list {
    use super::*;

    pub fn serialize<S: Serializer>(list: &[Vec<u8>], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(list.iter().map(|bytes| Bytes(bytes.clone())))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Vec<u8>>, D::Error> {
        let list = Vec::<Bytes>::deserialize(deserializer)?;
        Ok(list.into_iter().map(|bytes| bytes.0).collect())
    }
}

pub(crate) mod option_base64 {
    use super::*;

    pub fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        bytes.clone().map(Bytes).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Ok(Option::<Bytes>::deserialize(deserializer)?.map(|bytes| bytes.0))
    }
}
