//! What a site sends with its transactions: notes on their accounts and on
//! their instruction data, the order they run in and who sends them, as the
//! JSON of `docs/intents.md` writes them.

use super::text::{self, Address};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use solana_pubkey::Pubkey;

/// How the transactions of a request run together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    /// Everything runs in one transaction, or nothing does.
    Atomic,
    /// Each runs once the one before it succeeded. The first failure stops
    /// the rest.
    #[default]
    Serial,
    /// Each stands alone.
    Any,
}

/// Who sends the transactions once the wallet has signed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// The wallet composes, signs and sends.
    #[default]
    Send,
    /// The app adds its signatures after the wallet signs, and the wallet
    /// sends.
    Cosign,
    /// The wallet hands the signed transactions back, and the app signs and
    /// sends them.
    Return,
}

impl Delivery {
    /// The app signs after the wallet: `cosign` and `return`.
    pub fn app_signs(self) -> bool {
        matches!(self, Self::Cosign | Self::Return)
    }
}

/// Who holds a key the app signs with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignsBy {
    App,
}

/// What a note says about one account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AccountNote {
    /// The request creates this account, and the app made its key.
    New { replaceable: bool },
    /// The address follows from others.
    Derived(Derivation),
    /// This address pays. With `signs: "app"` the app holds its key.
    #[serde(rename_all = "camelCase")]
    FeePayer {
        replaceable: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signs: Option<SignsBy>,
    },
    /// A key the app holds signs the result.
    Signer { signs: SignsBy },
}

impl AccountNote {
    /// The wallet may put an address of its own in this one's place.
    pub fn replaceable(&self) -> bool {
        matches!(
            self,
            Self::New { replaceable: true }
                | Self::FeePayer {
                    replaceable: true,
                    ..
                }
        )
    }
}

/// How a `derived` address follows from others.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Derivation {
    /// A program-derived address, with its canonical bump.
    Pda(Pda),
    /// `createWithSeed`.
    WithSeed(WithSeed),
}

impl Derivation {
    /// Every address it rests on: the program and the seeds that are
    /// addresses, or the base and the owner.
    pub fn inputs(&self) -> Vec<Pubkey> {
        match self {
            Self::Pda(pda) => std::iter::once(pda.program)
                .chain(pda.seeds.iter().filter_map(|seed| match seed {
                    Seed::Account(account) => Some(*account),
                    Seed::Bytes(_) => None,
                }))
                .collect(),
            Self::WithSeed(with_seed) => vec![with_seed.base, with_seed.owner],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pda {
    #[serde(with = "text::address")]
    pub program: Pubkey,
    pub seeds: Vec<Seed>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithSeed {
    #[serde(with = "text::address")]
    pub base: Pubkey,
    pub seed: String,
    #[serde(with = "text::address")]
    pub owner: Pubkey,
}

/// One seed of a program-derived address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Seed {
    /// An address's 32 bytes.
    Account(#[serde(with = "text::address")] Pubkey),
    Bytes(#[serde(with = "text::base64_bytes")] Vec<u8>),
}

impl Seed {
    /// The seed's bytes, with an address taken from `address`.
    pub fn bytes_with(&self, address: impl Fn(&Pubkey) -> Pubkey) -> Vec<u8> {
        match self {
            Self::Account(account) => address(account).to_bytes().to_vec(),
            Self::Bytes(bytes) => bytes.clone(),
        }
    }
}

/// A place in a transaction's instruction data that holds an address or a
/// derived account's bump.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataNote {
    /// Which transaction of the request.
    pub transaction: usize,
    /// Which of its instructions, as it was sent, compute budget included.
    pub instruction: usize,
    /// Where in that instruction's data.
    pub offset: usize,
    #[serde(flatten)]
    pub holds: DataHolds,
}

/// What a [`DataNote`]'s bytes are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DataHolds {
    /// 32 bytes: that address.
    Account(#[serde(with = "text::address")] Pubkey),
    /// 1 byte: that derived account's bump.
    BumpOf(#[serde(with = "text::address")] Pubkey),
}

impl DataNote {
    /// The address the entry names.
    pub fn address(&self) -> Pubkey {
        match self.holds {
            DataHolds::Account(address) | DataHolds::BumpOf(address) => address,
        }
    }

    /// How many bytes it names.
    pub fn width(&self) -> usize {
        match self.holds {
            DataHolds::Account(_) => 32,
            DataHolds::BumpOf(_) => 1,
        }
    }
}

/// The `accounts` of a request: each address with its note, in the order
/// they were written. An address written twice stays twice, so the check
/// that an address carries one note can name it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountNotes(pub Vec<(Pubkey, AccountNote)>);

impl AccountNotes {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The note on `address`, the first when there are several.
    pub fn get(&self, address: &Pubkey) -> Option<&AccountNote> {
        self.0
            .iter()
            .find(|(key, _)| key == address)
            .map(|(_, note)| note)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Pubkey, &AccountNote)> {
        self.0.iter().map(|(address, note)| (address, note))
    }
}

impl FromIterator<(Pubkey, AccountNote)> for AccountNotes {
    fn from_iter<I: IntoIterator<Item = (Pubkey, AccountNote)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl Serialize for AccountNotes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(
            self.0
                .iter()
                .map(|(address, note)| (Address(*address), note)),
        )
    }
}

impl<'de> Deserialize<'de> for AccountNotes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Notes;
        impl<'de> Visitor<'de> for Notes {
            type Value = AccountNotes;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of notes keyed by address")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<AccountNotes, A::Error> {
                let mut notes = Vec::new();
                while let Some((address, note)) = map.next_entry::<Address, AccountNote>()? {
                    notes.push((address.0, note));
                }
                Ok(AccountNotes(notes))
            }
        }
        deserializer.deserialize_map(Notes)
    }
}

/// Every note of a request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notes {
    #[serde(default, skip_serializing_if = "AccountNotes::is_empty")]
    pub accounts: AccountNotes,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<DataNote>,
}

impl Notes {
    /// The derivation of `address`, when it carries a `derived` note.
    pub fn derivation(&self, address: &Pubkey) -> Option<&Derivation> {
        match self.accounts.get(address)? {
            AccountNote::Derived(derivation) => Some(derivation),
            _ => None,
        }
    }
}

/// A request as a site sends it: `ResolveInput` of `docs/intents.md`, with
/// the transactions as base64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveInput {
    /// The app's own, `[A-Za-z0-9-]{1,80}`. See [`valid_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The account acted as.
    #[serde(with = "text::address")]
    pub account: Pubkey,
    pub chain: String,
    #[serde(with = "text::base64_list")]
    pub transactions: Vec<Vec<u8>>,
    #[serde(default)]
    pub order: Order,
    #[serde(default)]
    pub delivery: Delivery,
    #[serde(flatten)]
    pub notes: Notes,
}

/// The most transactions a request carries.
pub const MAX_TRANSACTIONS: usize = 16;

/// Whether an app's id is one a request may carry: 1 to 80 letters, digits
/// and dashes.
pub fn valid_id(id: &str) -> bool {
    (1..=80).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
