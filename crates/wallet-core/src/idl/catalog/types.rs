//! What the catalog records about each stored IDL.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IdlSpec {
    Anchor,
    Codama,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IdlOrigin {
    /// Shipped inside the app. Never written to the index.
    Builtin,
    File,
    /// JSON taken from the program-metadata `idl` account.
    ProgramMetadata,
    /// JSON taken from the legacy Anchor IDL account.
    Anchor,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdlCheck {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "option_hex32"
    )]
    pub remote_sha256: Option<[u8; 32]>,
    /// The chain publishes the stored bytes.
    pub same: bool,
    /// The chain publishes the document the stored file was made from
    /// ([`IdlRecord::source`]). The file is current, though its bytes differ.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub source: bool,
}

impl IdlCheck {
    /// The chain publishes this file, or what it was made from.
    pub fn current(&self) -> bool {
        self.same || self.source
    }
}

/// The on-chain document an IDL file was made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdlChainSource {
    /// Where it was read.
    pub network: IdlNetwork,
    /// The account it was read from: [`IdlOrigin::ProgramMetadata`] or
    /// [`IdlOrigin::Anchor`].
    pub account: IdlOrigin,
    /// Of the bytes read.
    pub sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdlRecord {
    #[serde(with = "pubkey_string")]
    pub program_id: Pubkey,
    pub name: String,
    #[serde(with = "hex32")]
    pub sha256: [u8; 32],
    pub spec: IdlSpec,
    pub origin: IdlOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<IdlCheck>,
    /// Set when this program is read from the `additionalPrograms` of another
    /// program's IDL, the one named here. Its checksum, origin and check are
    /// that file's. Never written to the index: it is worked out again from
    /// the stored files.
    #[serde(skip)]
    pub host: Option<Pubkey>,
    /// Set on an embedded file made from what a program publishes on chain.
    /// Never written to the index: it comes with the app.
    #[serde(skip)]
    pub source: Option<IdlChainSource>,
}

impl IdlRecord {
    /// The on-chain account a check compares this file with: the one it was
    /// made from, the one it was taken from, or the Anchor IDL account.
    pub fn compared_account(&self) -> IdlOrigin {
        match (self.source, self.origin) {
            (Some(source), _) => source.account,
            (None, IdlOrigin::ProgramMetadata) => IdlOrigin::ProgramMetadata,
            (None, _) => IdlOrigin::Anchor,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IdlIndex {
    pub(super) records: Vec<IndexedRecord>,
}

/// One stored file and the network it is kept for.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IndexedRecord {
    /// Missing in an index written before IDLs were kept per network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) network: Option<IdlNetwork>,
    #[serde(flatten)]
    pub(super) record: IdlRecord,
}

#[derive(Debug, Error)]
pub enum IdlError {
    #[error("IDL file is empty")]
    Empty,
    #[error("IDL file is not JSON")]
    NotJson,
    #[error("IDL is neither a Codama root nor an Anchor IDL")]
    Unrecognized,
    #[error("program id is required for this IDL")]
    ProgramIdRequired,
    #[error("IDL program id {file} does not match the given id {given}")]
    ProgramIdMismatch { file: Pubkey, given: Pubkey },
    #[error("{0}")]
    Invalid(String),
    #[error("bundle has no index.json")]
    BundleMissingIndex,
    #[error("{0} is built in and cannot be removed")]
    Builtin(String),
    #[error("{name} comes with the IDL of {host}. Remove that one instead")]
    Companion { name: String, host: String },
    #[error("{0}")]
    Store(String),
}

/// Result of importing a folder of IDL files described by `index.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdlBundleReport {
    pub imported: Vec<IdlRecord>,
    pub skipped: Vec<String>,
}

pub fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    sha256(bytes)
}

pub(super) fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

pub(super) mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("sha256 must be 32 bytes"))
    }
}

pub(super) mod option_hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        bytes: &Option<[u8; 32]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => serializer.serialize_some(&hex::encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<[u8; 32]>, D::Error> {
        let value = Option::<String>::deserialize(deserializer)?;
        value
            .map(|text| {
                let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
                bytes
                    .try_into()
                    .map_err(|_| serde::de::Error::custom("sha256 must be 32 bytes"))
            })
            .transpose()
    }
}
