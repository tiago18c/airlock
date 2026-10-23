//! What to call an address, and what kind of thing it is. Every screen asks
//! here, so an address reads the same wherever it appears.
//!
//! A wallet of this device wins over the address book: its name and its sort
//! come from the wallet list. The book names everything else. Nothing here
//! makes a label up from the address itself.

use crate::book::address_book::{AddressBook, AddressKind, Origin};
use crate::book::holdings::{Holdings, TokenFlag};
use crate::wallets::{WalletClass, WalletKind, WalletSummary};
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;

/// The sort of a wallet of this device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WalletSort {
    /// The key is in this vault.
    Hot,
    /// The key is on another device.
    Cold,
    Hardware,
    /// A program decides who may act: Squads, a programmatic signer, Vector.
    SmartContract,
}

/// What an address is, as the screens group it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Wallet(WalletSort),
    Nonce,
    Program,
    Validator,
    Token,
    Contact,
    /// Nothing on this device names it.
    Unknown,
}

impl Category {
    /// "Hot wallet", "Program", and so on.
    pub fn name(self) -> &'static str {
        match self {
            Self::Wallet(WalletSort::Hot) => "Hot wallet",
            Self::Wallet(WalletSort::Cold) => "Cold wallet",
            Self::Wallet(WalletSort::Hardware) => "Hardware wallet",
            Self::Wallet(WalletSort::SmartContract) => "Smart contract wallet",
            Self::Nonce => "Nonce account",
            Self::Program => "Program",
            Self::Validator => "Validator",
            Self::Token => "Token",
            Self::Contact => "Contact",
            Self::Unknown => "Not in the address book",
        }
    }
}

/// The groups the address book filters by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Wallets,
    Nonces,
    Programs,
    Validators,
    Tokens,
    Contacts,
}

impl Group {
    pub const ALL: [Group; 6] = [
        Group::Wallets,
        Group::Nonces,
        Group::Programs,
        Group::Validators,
        Group::Tokens,
        Group::Contacts,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Wallets => "Wallets",
            Self::Nonces => "Nonces",
            Self::Programs => "Programs",
            Self::Validators => "Validators",
            Self::Tokens => "Tokens",
            Self::Contacts => "Contacts",
        }
    }
}

impl Category {
    /// The group it is listed under. Nothing lists what is unknown.
    pub fn group(self) -> Option<Group> {
        Some(match self {
            Self::Wallet(_) => Group::Wallets,
            Self::Nonce => Group::Nonces,
            Self::Program => Group::Programs,
            Self::Validator => Group::Validators,
            Self::Token => Group::Tokens,
            Self::Contact => Group::Contacts,
            Self::Unknown => return None,
        })
    }
}

impl From<WalletClass> for WalletSort {
    fn from(class: WalletClass) -> Self {
        match class {
            WalletClass::Hot => Self::Hot,
            WalletClass::Offline => Self::Cold,
            WalletClass::Hardware => Self::Hardware,
            WalletClass::SmartContract => Self::SmartContract,
        }
    }
}

/// One thing known about an address, for the card that opens over it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fact {
    /// "seed · m/44'/501'/0'/0'", "Squads v4 · 2 of 3".
    WalletKind(String),
    Balance(u64),
    TokenAmount {
        amount: u64,
        decimals: u8,
        mint: Pubkey,
    },
    /// The owner of a token account, or the authority of a nonce account.
    OwnedBy(Pubkey),
    /// A multisig of this device lists it as a member.
    MemberOf(Pubkey),
    /// One of the addresses of a smart wallet that acts as another.
    PartOf {
        wallet: Pubkey,
        role: &'static str,
    },
    PaysFeesVia(Pubkey),
    Flag(TokenFlag),
    Origin(Origin),
}

/// An address, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Described {
    pub pubkey: Pubkey,
    pub category: Category,
    /// `None` when nothing names it. Never the address itself.
    pub label: Option<String>,
    /// The name of the imported group the label comes from.
    pub group: Option<String>,
    /// Other addresses shown with the same label.
    pub collides_with: Vec<Pubkey>,
    pub facts: Vec<Fact>,
}

impl Described {
    /// The label with its group, as "Team > Bob".
    pub fn full_label(&self) -> Option<String> {
        let label = self.label.as_ref()?;
        Some(match &self.group {
            Some(group) => format!("{group} > {label}"),
            None => label.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// The address the wallet acts as.
    Own,
    /// A Squads wallet's multisig account.
    Multisig,
}

/// Everything the resolver reads, built once per change of the wallets, the
/// book or the holdings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Directory {
    wallets: Vec<WalletSummary>,
    by_address: HashMap<Pubkey, (usize, Role)>,
    book: Arc<AddressBook>,
    holdings: Arc<Holdings>,
    programs: HashMap<Pubkey, String>,
    /// Addresses by the key of the label they are shown with.
    labels: HashMap<String, Vec<Pubkey>>,
}

impl Directory {
    pub fn new(wallets: &[WalletSummary], book: Arc<AddressBook>, holdings: Arc<Holdings>) -> Self {
        Self::with_programs(wallets, book, holdings, HashMap::new())
    }

    /// `programs` names programs the book does not, such as those an IDL
    /// on this device describes.
    pub fn with_programs(
        wallets: &[WalletSummary],
        book: Arc<AddressBook>,
        holdings: Arc<Holdings>,
        programs: HashMap<Pubkey, String>,
    ) -> Self {
        let mut by_address = HashMap::new();
        for (index, wallet) in wallets.iter().enumerate() {
            if let Some(address) = wallet.address {
                by_address.insert(address, (index, Role::Own));
            }
            if let Some(WalletKind::Squads { multisig, .. }) = &wallet.contract {
                by_address
                    .entry(*multisig)
                    .or_insert((index, Role::Multisig));
            }
        }
        let mut directory = Self {
            wallets: wallets.to_vec(),
            by_address,
            book,
            holdings,
            programs,
            labels: HashMap::new(),
        };
        directory.index_labels();
        directory
    }

    fn index_labels(&mut self) {
        let mut labels: HashMap<String, Vec<Pubkey>> = HashMap::new();
        let mut add = |key: String, pubkey: Pubkey| {
            let list = labels.entry(key).or_default();
            if !list.contains(&pubkey) {
                list.push(pubkey);
            }
        };
        for (address, (index, role)) in &self.by_address {
            let label = wallet_label(&self.wallets[*index], *role);
            if let Some(key) = label_key(None, &label) {
                add(key, *address);
            }
        }
        for row in self.book.rows("", None) {
            let pubkey = row.entry.pubkey;
            if self.by_address.contains_key(&pubkey) {
                continue;
            }
            // The same address in two groups shows the first one's label.
            let shown = self
                .book
                .place(&pubkey)
                .is_some_and(|place| place.group_id == row.group_id);
            if !shown {
                continue;
            }
            let group = row.imported.then_some(row.group_label.as_str());
            if let Some(key) = label_key(group, &row.entry.label) {
                add(key, pubkey);
            }
        }
        labels.retain(|_, list| list.len() > 1);
        self.labels = labels;
    }

    pub fn describe(&self, pubkey: &Pubkey) -> Described {
        let mut described = match self.by_address.get(pubkey) {
            Some((index, role)) => self.describe_wallet(*pubkey, &self.wallets[*index], *role),
            None => self.describe_other(*pubkey),
        };
        if let Some(key) = described
            .label
            .as_deref()
            .and_then(|label| label_key(described.group.as_deref(), label))
        {
            if let Some(list) = self.labels.get(&key) {
                described.collides_with = list.iter().copied().filter(|pk| pk != pubkey).collect();
            }
        }
        described
    }

    /// The label to show, when there is one.
    pub fn label_of(&self, pubkey: &Pubkey) -> Option<String> {
        self.describe(pubkey).label
    }

    /// Every label shared by more than one address, with those addresses.
    pub fn collisions(&self) -> Vec<(String, Vec<Pubkey>)> {
        let mut out: Vec<(String, Vec<Pubkey>)> = self
            .labels
            .values()
            .filter_map(|list| {
                let label = self.describe(list.first()?).full_label()?;
                Some((label, list.clone()))
            })
            .collect();
        out.sort();
        out
    }

    fn describe_wallet(&self, pubkey: Pubkey, wallet: &WalletSummary, role: Role) -> Described {
        let mut facts = Vec::new();
        if !wallet.summary.is_empty() {
            facts.push(Fact::WalletKind(wallet.summary.clone()));
        }
        if let (Role::Multisig, Some(address)) = (role, wallet.address) {
            facts.push(Fact::PartOf {
                wallet: address,
                role: "multisig",
            });
        }
        let entry = self.book.get(&pubkey);
        if let Some(lamports) = entry.and_then(|entry| entry.lamports) {
            facts.push(Fact::Balance(lamports));
        }
        if let Some(payer) = wallet
            .fee_payer
            .and_then(|id| self.wallets.iter().find(|other| other.id == id))
            .and_then(|payer| payer.address)
        {
            facts.push(Fact::PaysFeesVia(payer));
        }
        for multisig in &self.wallets {
            if let Some(WalletKind::Squads { members, .. }) = &multisig.contract {
                if members.contains(&pubkey) {
                    if let Some(vault) = multisig.address {
                        facts.push(Fact::MemberOf(vault));
                    }
                }
            }
        }
        Described {
            pubkey,
            category: Category::Wallet(wallet.class.into()),
            label: Some(wallet_label(wallet, role)).filter(|label| !label.trim().is_empty()),
            group: None,
            collides_with: Vec::new(),
            facts,
        }
    }

    fn describe_other(&self, pubkey: Pubkey) -> Described {
        let place = self.book.place(&pubkey);
        let entry = place.as_ref().map(|place| &place.entry);
        let held = self.holdings.find(&pubkey);
        let mut facts = Vec::new();
        let category = match entry {
            Some(entry) => match entry.kind {
                // A key of this device that the wallets do not list.
                AddressKind::Account if entry.wallet_id.is_some() => {
                    Category::Wallet(WalletSort::Hot)
                }
                AddressKind::Account => Category::Contact,
                AddressKind::TokenAccount | AddressKind::Mint => Category::Token,
                AddressKind::NonceAccount => Category::Nonce,
                AddressKind::Program => Category::Program,
                AddressKind::Validator => Category::Validator,
            },
            None if held.is_some() => Category::Token,
            None if self.programs.contains_key(&pubkey) => Category::Program,
            None => Category::Unknown,
        };
        let mut label = entry
            .map(|entry| entry.label.trim().to_string())
            .filter(|label| !label.is_empty());
        let group = place
            .as_ref()
            .filter(|place| place.imported && label.is_some())
            .and_then(|place| place.group_label.clone());

        if let Some(lamports) = entry.and_then(|entry| entry.lamports) {
            facts.push(Fact::Balance(lamports));
        }
        match (held, entry) {
            (Some((owner, holding)), _) => {
                facts.push(Fact::TokenAmount {
                    amount: holding.amount,
                    decimals: holding.decimals,
                    mint: holding.mint,
                });
                facts.push(Fact::OwnedBy(owner));
                if let Some(flag) = holding.flag {
                    facts.push(Fact::Flag(flag));
                }
                if label.is_none() {
                    label = self.mint_label(&holding.mint);
                }
            }
            (None, Some(entry)) if entry.kind == AddressKind::TokenAccount => {
                if let (Some(amount), Some(decimals), Some(mint)) =
                    (entry.token_amount, entry.decimals, entry.mint)
                {
                    facts.push(Fact::TokenAmount {
                        amount,
                        decimals,
                        mint,
                    });
                }
                if let Some(owner) = entry.owner {
                    facts.push(Fact::OwnedBy(owner));
                }
                if label.is_none() {
                    label = entry.mint.and_then(|mint| self.mint_label(&mint));
                }
            }
            (None, Some(entry)) if entry.kind == AddressKind::NonceAccount => {
                if let Some(authority) = entry.owner {
                    facts.push(Fact::OwnedBy(authority));
                    if label.is_none() {
                        label = self
                            .explicit_label(&authority)
                            .map(|name| format!("Nonce of {name}"));
                    }
                }
            }
            _ => {}
        }
        if label.is_none() {
            label = self.programs.get(&pubkey).cloned();
        }
        if let Some(entry) = entry {
            facts.push(Fact::Origin(entry.origin));
        }
        Described {
            pubkey,
            category,
            label,
            group,
            collides_with: Vec::new(),
            facts,
        }
    }

    fn mint_label(&self, mint: &Pubkey) -> Option<String> {
        self.explicit_label(mint)
    }

    /// A name someone gave: a wallet's, or the book's. Never derived.
    fn explicit_label(&self, pubkey: &Pubkey) -> Option<String> {
        if let Some((index, role)) = self.by_address.get(pubkey) {
            return Some(wallet_label(&self.wallets[*index], *role))
                .filter(|label| !label.trim().is_empty());
        }
        self.book
            .get(pubkey)
            .map(|entry| entry.label.trim().to_string())
            .filter(|label| !label.is_empty())
    }
}

fn wallet_label(wallet: &WalletSummary, role: Role) -> String {
    match role {
        Role::Own => wallet.label.trim().to_string(),
        Role::Multisig => format!("{} multisig", wallet.label.trim()),
    }
}

/// Labels compare trimmed and without case. An imported label carries its
/// group, so "Team > Bob" and a local "Bob" do not collide.
fn label_key(group: Option<&str>, label: &str) -> Option<String> {
    let label = label.trim().to_lowercase();
    if label.is_empty() {
        return None;
    }
    Some(match group {
        Some(group) => format!("{} > {label}", group.trim().to_lowercase()),
        None => label,
    })
}

#[cfg(test)]
#[path = "describe_tests.rs"]
mod tests;
