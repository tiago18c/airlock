use crate::ids::WalletId;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::wallets::squads::{Squad, SquadsProgram};
use crate::wallets::vector::SchemeId;
use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use solana_pubkey::Pubkey;
use std::collections::BTreeSet;
use wallet_vault::{SecretBytes, SecretString};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HardwareVendor {
    Ledger,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletRecord {
    pub id: WalletId,
    pub label: String,
    pub tags: BTreeSet<String>,
    pub kind: WalletKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WalletKind {
    /// An offline wallet: the key is kept on another device, which signs
    /// what this one prepares.
    Offline {
        pubkey: Pubkey,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fee_payer: Option<WalletId>,
        /// A system nonce account whose authority is `pubkey`. A transaction
        /// this key signs for itself lives on it, so it stays valid however
        /// long the trip to the other device takes. Deprecated once the
        /// programmatic signer or Vector is on mainnet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        durable_nonce: Option<Pubkey>,
        /// Which path acts when this key is chosen. `None` is the key
        /// itself, on whatever lifetime it has.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_path: Option<OfflinePath>,
    },
    Keypair {
        /// 32-byte ed25519 seed (not the 64-byte keypair bytes).
        secret_seed: SecretBytes,
        /// The address of the key, so a copy without the seed still has it.
        /// Missing in records from before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pubkey: Option<Pubkey>,
    },
    Seed {
        mnemonic: SecretString,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        passphrase: Option<SecretString>,
        /// Concrete path, such as `m/44'/501'/0'/0'`.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pubkey: Option<Pubkey>,
    },
    Hardware {
        vendor: HardwareVendor,
        #[serde(default)]
        paths: Vec<String>,
    },
    /// A programmatic signer: an address with no key of its own, that a
    /// program lets an authority key act for. The authority signs an
    /// authorization, never a transaction, so it needs no funds and can stay
    /// offline. A hot wallet relays what it authorized.
    Programmatic {
        /// The program that stands behind the address, and what it needs.
        protocol: Protocol,
        /// Where the authority key is.
        authority: Authority,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fee_payer: Option<WalletId>,
    },
    Squads {
        #[serde(default)]
        program: SquadsProgram,
        /// The multisig account (v4) or the settings account (smart account).
        multisig: Pubkey,
        /// Where the funds are.
        vault: Pubkey,
        #[serde(default)]
        members: Vec<Pubkey>,
        #[serde(default)]
        threshold: u16,
        /// The wallet on this device that acts as a member.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        member_wallet: Option<WalletId>,
        /// Other wallets on this device that approve together with the acting
        /// member, in one transaction.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        approves_with: Vec<WalletId>,
        /// The durable nonce account a combined approval advances when an offline
        /// wallet votes in it. Its authority has to be the fee payer or one of the
        /// plain keys that vote.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_nonce: Option<Pubkey>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fee_payer: Option<WalletId>,
    },
}

/// The program behind a programmatic signer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "camelCase")]
pub enum Protocol {
    /// `solana-program/ed25519-programmatic-signer`. The address is derived
    /// from `authority` and never becomes an account.
    Ed25519Signer {
        authority: Pubkey,
        /// The executor nonce account that orders this signer's actions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<Pubkey>,
    },
    /// A Vector account: an account of the Vector program, which has to be
    /// initialized, controlled by a key of `scheme`.
    Vector {
        scheme: SchemeId,
        /// The program this wallet was made under. The address is derived
        /// under it, so a wallet does not move when a program is deployed
        /// again somewhere else.
        program: Pubkey,
        /// The key the account is created with. It names the account for
        /// life: for a scheme whose key is replaced, this is the first one.
        key: PublicBytes,
        /// The next XMSS public key to rotate into, when another device
        /// (or this one) has exported it. Cleared after the new key is
        /// installed in this record.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_pubkey: Option<PublicBytes>,
        /// The seed that belongs with `next_pubkey`, when this device made
        /// it. Installed into `Authority::Held` after the rotation lands.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_secret: Option<SecretBytes>,
        /// The seed `Authority::Held` had before the last rotation was
        /// installed. Never signs: it is kept for when a device installed a
        /// rotation that had not landed, which would otherwise leave the
        /// account held by a key nobody has.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retired_secret: Option<SecretBytes>,
        /// The recovery phrase the keys are made from, for a key kept here
        /// that moves on. `Authority::Held` keeps the seed of the key that
        /// signs now, which is key number `index` of the phrase.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phrase: Option<KeyPhrase>,
    },
}

/// The recovery phrase a Vector wallet's hash-based keys are made from, and
/// which of its keys signs now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyPhrase {
    pub words: SecretString,
    /// The number of the key that signs now.
    pub index: u32,
    /// Made again from the phrase on this device. A lost copy may have
    /// signed with leaves of the key the account holds that the chain has
    /// not seen, so nothing is signed with it but a move to the next key,
    /// at its last leaf.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restored: bool,
}

/// The two ways an offline key acts without its transaction expiring on the
/// way to the other device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfflinePath {
    /// Its programmatic signer acts: a wallet of its own, at another address,
    /// whose authorizations wait on an executor nonce.
    Programmatic,
    /// The key itself acts, on its durable nonce. Deprecated once the
    /// programmatic signer or Vector is on mainnet.
    DurableNonce,
}

/// Where the key that signs a programmatic signer's authorizations is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", rename_all = "camelCase")]
pub enum Authority {
    /// It is a wallet of this device: a hot wallet that holds the key, or an
    /// offline wallet when another device does. An Ed25519 key only.
    Wallet { wallet: WalletId },
    /// It is kept in this record. A key of a scheme other than Ed25519 has
    /// no address and pays no fee, so it is not a wallet of its own. Only a
    /// Vector account has one.
    Held {
        secret: SecretBytes,
        /// The next leaf to sign with, for a scheme whose key signs once per
        /// leaf. Zero otherwise.
        next_leaf: u32,
    },
    /// It is somewhere this app does not manage.
    Outside,
}

impl Authority {
    /// The wallet of this device that stands for the key, when one does.
    pub fn of(wallet: Option<WalletId>) -> Self {
        match wallet {
            Some(wallet) => Self::Wallet { wallet },
            None => Self::Outside,
        }
    }

    pub fn wallet(&self) -> Option<WalletId> {
        match self {
            Self::Wallet { wallet } => Some(*wallet),
            Self::Held { .. } | Self::Outside => None,
        }
    }
}

/// What a Vector wallet is controlled by, borrowed from its record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VectorSettings<'a> {
    pub scheme: SchemeId,
    /// The key the account was created with.
    pub key: &'a [u8],
    pub authority: &'a Authority,
}

/// Bytes that are no secret, such as a public key too long to be an address.
/// Stored as base64.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicBytes(pub Vec<u8>);

impl std::fmt::Debug for PublicBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PublicBytes({} bytes)", self.0.len())
    }
}

impl Serialize for PublicBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for PublicBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// How a wallet is grouped on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WalletClass {
    /// The key is in this vault.
    Hot,
    Hardware,
    /// An on-chain program decides who may act: Squads, a programmatic signer.
    SmartContract,
    /// The key is on another device.
    Offline,
}

impl WalletRecord {
    /// An offline wallet: `pubkey` is a key kept on another device.
    pub fn offline(label: impl Into<String>, pubkey: Pubkey) -> Self {
        Self {
            id: WalletId::new(),
            label: label.into(),
            tags: BTreeSet::new(),
            kind: WalletKind::Offline {
                pubkey,
                fee_payer: None,
                durable_nonce: None,
                default_path: None,
            },
        }
    }

    pub fn from_keypair_seed(label: impl Into<String>, secret_seed: [u8; 32]) -> Self {
        let pubkey = KeypairSigner::from_seed(&secret_seed)
            .ok()
            .map(|signer| signer.pubkey());
        Self {
            id: WalletId::new(),
            label: label.into(),
            tags: BTreeSet::new(),
            kind: WalletKind::Keypair {
                secret_seed: SecretBytes(secret_seed.to_vec()),
                pubkey,
            },
        }
    }

    pub fn from_seed_phrase(
        label: impl Into<String>,
        mnemonic: impl Into<String>,
        passphrase: Option<String>,
        path: impl Into<String>,
        pubkey: Pubkey,
    ) -> Self {
        let passphrase = passphrase.and_then(|value| {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(SecretString(trimmed))
            }
        });
        Self {
            id: WalletId::new(),
            label: label.into(),
            tags: BTreeSet::new(),
            kind: WalletKind::Seed {
                mnemonic: SecretString(mnemonic.into()),
                passphrase,
                path: path.into(),
                pubkey: Some(pubkey),
            },
        }
    }

    /// An Ed25519 programmatic signer for `authority`. `authority_wallet` is
    /// the wallet of this device that stands for that key, if one does.
    pub fn programmatic(
        label: impl Into<String>,
        authority: Pubkey,
        authority_wallet: Option<WalletId>,
    ) -> Self {
        Self {
            id: WalletId::new(),
            label: label.into(),
            tags: BTreeSet::new(),
            kind: WalletKind::Programmatic {
                protocol: Protocol::Ed25519Signer {
                    authority,
                    nonce: None,
                },
                authority: Authority::of(authority_wallet),
                fee_payer: None,
            },
        }
    }

    /// A Vector account of `key` under `scheme`, in the program at `program`.
    pub fn vector(
        label: impl Into<String>,
        scheme: SchemeId,
        program: Pubkey,
        key: Vec<u8>,
        authority: Authority,
    ) -> Self {
        Self {
            id: WalletId::new(),
            label: label.into(),
            tags: BTreeSet::new(),
            kind: WalletKind::Programmatic {
                protocol: Protocol::Vector {
                    scheme,
                    program,
                    key: PublicBytes(key),
                    next_pubkey: None,
                    next_secret: None,
                    retired_secret: None,
                    phrase: None,
                },
                authority,
                fee_payer: None,
            },
        }
    }

    /// A Squads multisig, spending from its first vault.
    pub fn squads(
        label: impl Into<String>,
        squad: Squad,
        members: Vec<Pubkey>,
        threshold: u16,
    ) -> Self {
        Self {
            id: WalletId::new(),
            label: label.into(),
            tags: BTreeSet::new(),
            kind: WalletKind::Squads {
                program: squad.program,
                multisig: squad.address,
                vault: squad.vault(0),
                members,
                threshold,
                member_wallet: None,
                approves_with: Vec::new(),
                approval_nonce: None,
                fee_payer: None,
            },
        }
    }

    /// What kind of wallet this is, in a few words for a list.
    pub fn summary(&self) -> String {
        match &self.kind {
            WalletKind::Offline { .. } => "offline key".into(),
            WalletKind::Keypair { .. } => "private key".into(),
            WalletKind::Seed { path, .. } => {
                if path.is_empty() {
                    "seed".into()
                } else {
                    format!("seed · {path}")
                }
            }
            WalletKind::Hardware { .. } => "hardware".into(),
            WalletKind::Programmatic {
                protocol: Protocol::Ed25519Signer { .. },
                ..
            } => "programmatic signer".into(),
            WalletKind::Programmatic {
                protocol: Protocol::Vector { scheme, .. },
                ..
            } => format!("Vector wallet · {}", scheme.label()),
            WalletKind::Squads {
                program,
                members,
                threshold,
                ..
            } => {
                let name = match program {
                    SquadsProgram::V4 => "Squads v4",
                    SquadsProgram::SmartAccount => "Squads smart account",
                };
                if members.is_empty() {
                    name.into()
                } else {
                    format!("{name} · {threshold} of {}", members.len())
                }
            }
        }
    }
}
