//! What to call an address, and what kind of thing it is. Every screen asks
//! here, so an address reads the same wherever it appears.
//!
//! A wallet of this device wins over the address book: its name and its sort
//! come from the wallet list. The book names everything else. Nothing here
//! makes a label up from the address itself.

use crate::book::address_book::{AddressBook, AddressKind, Origin};
use crate::book::holdings::{Holdings, TokenFlag};
use crate::idl::builtin::ED25519_SIGNER_PROGRAM_ID;
use crate::ids::{
    COMPUTE_BUDGET_PROGRAM_ID, MEMO_PROGRAM_ID, REALMS_GOVERNANCE_PROGRAM_ID,
    SQUADS_SMART_ACCOUNT_PROGRAM_ID, SQUADS_V4_PROGRAM_ID,
};
use crate::wallets::vector::PROGRAM_ID as VECTOR_PROGRAM_ID;
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
            Self::Wallet(WalletSort::Cold) => "Offline wallet",
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
    /// on this device describes. A program whose IDL ships with the app is
    /// shown by its display name rather than the IDL's.
    pub fn with_programs(
        wallets: &[WalletSummary],
        book: Arc<AddressBook>,
        holdings: Arc<Holdings>,
        mut programs: HashMap<Pubkey, String>,
    ) -> Self {
        for (pubkey, name) in programs.iter_mut() {
            if let Some(shown) = program_display_name(pubkey) {
                *name = shown.to_string();
            }
        }
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
                    label = self.token_account_label(Some(owner), Some(holding.mint));
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
                    label = self.token_account_label(entry.owner, entry.mint);
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

    /// A token account nothing names: whose it is and which token, as
    /// "Alice · USDC", when its owner has a name. Otherwise the mint's name,
    /// or none.
    fn token_account_label(&self, owner: Option<Pubkey>, mint: Option<Pubkey>) -> Option<String> {
        let token = mint.and_then(|mint| self.mint_label(&mint));
        match owner.and_then(|owner| self.explicit_label(&owner)) {
            Some(owner) => Some(format!(
                "{owner} · {}",
                token.as_deref().unwrap_or("token account")
            )),
            None => token,
        }
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

/// The names of the programs whose IDLs ship with the app, for where the
/// IDL's own name is an identifier ("computeBudget"). The System, Stake,
/// Token, Token-2022 and Associated Token programs are named by the address
/// book's seeded entries, which win over these.
const PROGRAM_DISPLAY_NAMES: &[(Pubkey, &str)] = &[
    (COMPUTE_BUDGET_PROGRAM_ID, "Compute Budget Program"),
    (ED25519_SIGNER_PROGRAM_ID, "Programmatic Signer Program"),
    (
        Pubkey::from_str_const("ExecxgyHYsAXB4c5dZodV1zJZ9hqfsDCYkRDRATrpkFR"),
        "Message Executor Program",
    ),
    (
        Pubkey::from_str_const("Noncediea1fH12usShuQAz28UhgAeuE5Maf32LsMUQB"),
        "Message Executor Nonce Program",
    ),
    (SQUADS_V4_PROGRAM_ID, "Squads Multisig Program"),
    (
        SQUADS_SMART_ACCOUNT_PROGRAM_ID,
        "Squads Smart Account Program",
    ),
    (VECTOR_PROGRAM_ID, "Vector Program"),
    // The three memo programs read the same data. The shipped files declare
    // v1 and v4 for the `Memo1…` and `Memo4…` addresses; the code calls the
    // one in between, the one the app writes memos with, both v2 and v3, so
    // it carries no number.
    (
        Pubkey::from_str_const("Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo"),
        "Memo Program v1",
    ),
    (MEMO_PROGRAM_ID, "Memo Program"),
    (
        Pubkey::from_str_const("Memo4c2pN8afCj432Lb7RMVKi9PbQnnW7ewFFaV3oAH"),
        "Memo Program v4",
    ),
    (
        Pubkey::from_str_const("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc"),
        "Orca Whirlpool Program",
    ),
    (
        Pubkey::from_str_const("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"),
        "Raydium CLMM Program",
    ),
    (
        Pubkey::from_str_const("DRayAUgENGQBKVaX8owNhgzkEDyoHTGVEGHVJT1E9pfH"),
        "Raydium CLMM Program",
    ),
    (
        Pubkey::from_str_const("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"),
        "Raydium CP Swap Program",
    ),
    (
        Pubkey::from_str_const("DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb"),
        "Raydium CP Swap Program",
    ),
    (
        Pubkey::from_str_const("DRaybByLpbUL57LJARs3j8BitTxVfzBg351EaMr5UTCd"),
        "Raydium Router Program",
    ),
    (REALMS_GOVERNANCE_PROGRAM_ID, "SPL Governance Program"),
    (
        Pubkey::from_str_const("ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S"),
        "Program Metadata Program",
    ),
    (
        Pubkey::from_str_const("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo"),
        "Meteora DLMM Program",
    ),
    (
        Pubkey::from_str_const("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG"),
        "Meteora DAMM v2 Program",
    ),
    (
        Pubkey::from_str_const("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ"),
        "Pump Fees Program",
    ),
    (
        Pubkey::from_str_const("De1egAFMkMWZSN5rYXRj9CAdheBamobVNubTsi9avR44"),
        "Subscriptions Program",
    ),
    (
        Pubkey::from_str_const("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"),
        "PumpSwap AMM Program",
    ),
    (
        Pubkey::from_str_const("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"),
        "Pump.fun Program",
    ),
    (
        Pubkey::from_str_const("EtrnLzgbS7nMMy5fbD42kXiUzGg8XQzJ972Xtk1cjWih"),
        "Phoenix Perpetuals Program",
    ),
    (
        Pubkey::from_str_const("DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH"),
        "DFlow Swap Program",
    ),
    (
        Pubkey::from_str_const("PERPHjGBqRHArX4DySjwM6UJHiR3sWAatqfdBS2qQJu"),
        "Jupiter Perpetuals Program",
    ),
    (
        Pubkey::from_str_const("KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD"),
        "Kamino Lend Program",
    ),
    (
        Pubkey::from_str_const("proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u"),
        "OKX DEX Router Program",
    ),
    (
        Pubkey::from_str_const("LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj"),
        "Raydium LaunchLab Program",
    ),
    (
        Pubkey::from_str_const("MarBmsSgKXdrN1egZf5sqe1TMai9K1rChYNDJgjq7aD"),
        "Marinade Liquid Staking Program",
    ),
    (
        Pubkey::from_str_const("dbcij3LWUppWqq96dh6gJWwBifmcGfLSB5D4DuSMaqN"),
        "Meteora Dynamic Bonding Curve Program",
    ),
    (
        Pubkey::from_str_const("jup3YeL8QhtSx1e253b2FDvsMNC87fDrgQZivbrndc9"),
        "Jupiter Lend Earn Program",
    ),
    (
        Pubkey::from_str_const("HpNfyc2Saw7RKkQd8nEL4khUcuPhQ7WwY1B2qjx8jxFq"),
        "PancakeSwap v3 Program",
    ),
    (
        Pubkey::from_str_const("jupr81YtYssSyPt8jbnGuiWon5f6x9TcDEFxYe3Bdzi"),
        "Jupiter Lend Borrow Program",
    ),
    (
        Pubkey::from_str_const("MFv2hWf31Z9kbCa1snEPYctwafyhdvnV7FZnsebVacA"),
        "marginfi Program",
    ),
    (
        Pubkey::from_str_const("FarmsPZpWu9i7Kky8tPN37rs2TpmMrAZrC7S7vJa91Hr"),
        "Kamino Farms Program",
    ),
    (
        Pubkey::from_str_const("KvauGMspG5k6rtzrqqn7WNn3oZdyKqLKwK2XWQ8FLjd"),
        "Kamino Vault Program",
    ),
    (
        Pubkey::from_str_const("6LtLpnUFNByNXLyCoK9wA2MykKAmQNZKBdY8s47dehDc"),
        "Kamino Liquidity Program",
    ),
    (
        Pubkey::from_str_const("onreuGhHHgVzMWSkj2oQDLDtvvGvoepBPkqyaubFcwe"),
        "OnRe Program",
    ),
    (
        Pubkey::from_str_const("Ccip842gzYHhvdDkSyi2YVCoAWPbYJoApMFzSxQroE9C"),
        "Chainlink CCIP Router Program",
    ),
    (
        Pubkey::from_str_const("SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy"),
        "SPL Stake Pool Program",
    ),
    (
        Pubkey::from_str_const("SP12tWFxD9oJsVWNavTTBZvMbA6gkAmxtVgxdqvyvhY"),
        "Sanctum SPL Stake Pool Program",
    ),
    (
        Pubkey::from_str_const("SPMBzsVUuoHA4Jm6KunbsotaahvVikZs1JyTW6iJvbn"),
        "Sanctum Multi-Validator Stake Pool Program",
    ),
    (
        Pubkey::from_str_const("1oopBoJG58DgkUVKkEzKgyG9dvRmpgeEm1AVjoHkF78"),
        "Loopscale Program",
    ),
    (
        Pubkey::from_str_const("HumaXepHnjaRCpjYTokxY4UtaJcmx41prQ8cxGmFC5fn"),
        "Huma Permissionless Program",
    ),
    (
        Pubkey::from_str_const("EVQ4s1b6N1vmWFDv8PRNc77kufBP8HcrSNWXQAhRsJq9"),
        "Huma Program",
    ),
    (
        Pubkey::from_str_const("HYEXCHtHkBagdStcJCp3xbbb9B7sdMdWXFNj6mdsG4hn"),
        "Hylo Exchange Program",
    ),
    (
        Pubkey::from_str_const("hyRouTRDAgn65xyyJ3L5c4k5SFmSdr3NxDV8Euzjy3f"),
        "Hylo Router Program",
    ),
    (
        Pubkey::from_str_const("HysTabVUfmQBFcmzu1ctRd1Y1fxd66RBpboy1bmtDSQQ"),
        "Hylo Earn Pool Program",
    ),
    (
        Pubkey::from_str_const("24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi"),
        "Meteora Dynamic Vault Program",
    ),
    (
        Pubkey::from_str_const("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"),
        "Raydium AMM v4 Program",
    ),
    (
        Pubkey::from_str_const("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB"),
        "Meteora DAMM v1 Program",
    ),
    (
        Pubkey::from_str_const("MNFSTqtC93rEfYHB6hF82sKdZpUDFWkViLByLd1k1Ms"),
        "Manifest Program",
    ),
    (
        Pubkey::from_str_const("REALQqNEomY6cQGZJUGwywTBD2UmDT32rZcNnfxQ5N2"),
        "Byreal CLMM Program",
    ),
    (
        Pubkey::from_str_const("jupeiUmn818Jg1ekPURTpr4mFo29p46vygyykFJ3wZC"),
        "Jupiter Lend Liquidity Program",
    ),
    (
        Pubkey::from_str_const("jupZ4m2GqUCJ5iueMfzQf8khFfH31d4XAQt3RzCT9Vd"),
        "Jupiter Lend DEX Program",
    ),
    (
        Pubkey::from_str_const("strmRqUCoQUgGUan5YhzUZa6KqdzwX5L6FpUxfmKg5m"),
        "Streamflow Program",
    ),
    (
        Pubkey::from_str_const("STAKEvGqQTtzJZH6BWDcbpzXXn2BBerPAgQ3EGLN2GH"),
        "Streamflow Staking Program",
    ),
    (
        Pubkey::from_str_const("mmm3XBJg5gk8XJxEKBvdgptZz6SgK4tXvn36sodowMc"),
        "Magic Eden AMM Program",
    ),
    (
        Pubkey::from_str_const("FUTARELBfJfQ8RDGhg1wdhddq1odMAJUePHFuBYfUxKq"),
        "MetaDAO Futarchy Program",
    ),
    (
        Pubkey::from_str_const("routeUGWgWzqBWFcrCfv8tritsqukccJPu3q5GPP3xS"),
        "Raydium Router Program",
    ),
    (
        Pubkey::from_str_const("So1endDq2YkqhipRh3WViPa8hdiSpxWy6z3Z6tMCpAo"),
        "Save Lending Program",
    ),
    (
        Pubkey::from_str_const("Vau1t6sLNxnzB7ZDsef8TLbPLfyZMYXH8WTNqUdm9g8"),
        "Jito Vault Program",
    ),
    (
        Pubkey::from_str_const("RestkWeAVL8fRGgzhfeoqFhsqKRchg6aa1XrcH96z4Q"),
        "Jito Restaking Program",
    ),
    (
        Pubkey::from_str_const("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s"),
        "Metaplex Token Metadata Program",
    ),
    (
        Pubkey::from_str_const("Gmso1uvJnLbawvw7yezdfCDcPydwW2s2iqG3w6MDucLo"),
        "GMX Solana Store Program",
    ),
    (
        Pubkey::from_str_const("8eqh8wppT9c5rw4ERqNCffvU6cNFJWff9WmkcYtmGiqC"),
        "Chainlink CCIP Lock-Release Pool Program",
    ),
    (
        Pubkey::from_str_const("76y77prsiCMvXMjuoZ5VRrhG5qYBrUMYTE5WgHqgjEn6"),
        "LayerZero Endpoint Program",
    ),
    (
        Pubkey::from_str_const("5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx"),
        "Sanctum Infinity Program",
    ),
    (
        Pubkey::from_str_const("phDEVv4w6BcfkLrLNeXr8HhhgQxnxziVGXpGPcaadMf"),
        "Phoenix Perpetuals Beta Program",
    ),
    (
        Pubkey::from_str_const("EMBERpYNE6ehWmXymZZS2skiFmCa9V5dp14e1iduM5qy"),
        "Phoenix Ember Program",
    ),
    (
        Pubkey::from_str_const("4R3gSG8BpU4t19KYj8CfnbtRpnT8gtk4dvTHxVRwc2r7"),
        "Jito Tip Distribution Program",
    ),
    (
        Pubkey::from_str_const("FD1amxhTsDpwzoVX41dxp2ygAESURV2zdUACzxM1Dfw9"),
        "Coinflow Program",
    ),
];

/// The display name of a program whose IDL ships with the app.
fn program_display_name(pubkey: &Pubkey) -> Option<&'static str> {
    PROGRAM_DISPLAY_NAMES
        .iter()
        .find(|(known, _)| known == pubkey)
        .map(|(_, name)| *name)
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
