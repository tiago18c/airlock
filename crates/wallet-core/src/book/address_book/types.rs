//! An address-book entry, and the views of the book the screens list.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    Imported,
    Derived,
    Discovered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AddressKind {
    /// A plain account: someone's address. Whether it is a wallet of this
    /// device comes from the wallet list, not from here.
    #[serde(alias = "owner", alias = "contact")]
    Account,
    TokenAccount,
    Mint,
    NonceAccount,
    Program,
    /// A vote account.
    Validator,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressEntry {
    #[serde(with = "crate::ids::pubkey_string")]
    pub pubkey: Pubkey,
    pub label: String,
    pub tags: BTreeSet<String>,
    pub kind: AddressKind,
    pub origin: Origin,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::option_pubkey_string"
    )]
    pub owner: Option<Pubkey>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::option_pubkey_string"
    )]
    pub mint: Option<Pubkey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet_id: Option<WalletId>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lamports: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_amount: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce_hash: Option<solana_hash::Hash>,
    /// Counted in the portfolio with the wallets of this device.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub watched: bool,
}

/// One namespace of addresses. The local group is this device; each import is its own group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BookGroup {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) entries: BTreeMap<String, AddressEntry>,
}

impl BookGroup {
    pub(super) fn this_device() -> Self {
        Self {
            id: LOCAL_ID.to_string(),
            label: "This device".to_string(),
            entries: BTreeMap::new(),
        }
    }
}

/// Plaintext file moved from one machine to another. Importing it adds a group;
/// it does not merge into the local book.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressBookFile {
    pub label: String,
    pub entries: Vec<AddressEntry>,
}

/// One address as shown in a group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookRow {
    pub group_id: String,
    pub group_label: String,
    pub imported: bool,
    pub entry: AddressEntry,
}

/// A group in the sidebar. The local group has no label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookGroupInfo {
    pub id: String,
    pub label: Option<String>,
    pub imported: bool,
    pub entries: usize,
}

/// Where an address was found. Local wins when the same key is in an import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressPlace {
    pub group_id: String,
    pub imported: bool,
    pub group_label: Option<String>,
    pub entry: AddressEntry,
}
