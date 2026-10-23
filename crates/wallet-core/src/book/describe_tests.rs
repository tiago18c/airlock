use super::*;
use crate::chain::DiscoveredTokenAccount;
use crate::wallets::squads::Squad;
use crate::wallets::vector::{self, SchemeId};
use crate::wallets::{Authority, VaultData, WalletRecord};
use crate::AddressBookFile;

fn pk(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

fn directory(wallets: Vec<WalletRecord>, book: AddressBook, holdings: Holdings) -> Directory {
    let data = VaultData {
        wallets,
        selected: None,
    };
    Directory::new(&data.summaries(), Arc::new(book), Arc::new(holdings))
}

fn book_only(book: AddressBook) -> Directory {
    directory(Vec::new(), book, Holdings::default())
}

#[test]
fn each_wallet_gets_its_sort() {
    let hot = WalletRecord::from_keypair_seed("Main", [7; 32]);
    let hot_address = hot.address().unwrap();
    let seed = WalletRecord::from_seed_phrase("Seeded", "words", None, "m/44'/501'/0'/0'", pk(2));
    let cold = WalletRecord::offline("Ops laptop", pk(3));
    let found = directory(
        vec![hot, seed, cold],
        AddressBook::new(),
        Holdings::default(),
    );

    let main = found.describe(&hot_address);
    assert_eq!(main.category, Category::Wallet(WalletSort::Hot));
    assert_eq!(main.label.as_deref(), Some("Main"));
    assert_eq!(
        found.describe(&pk(2)).category,
        Category::Wallet(WalletSort::Hot)
    );
    let cold = found.describe(&pk(3));
    assert_eq!(cold.category, Category::Wallet(WalletSort::Cold));
    assert_eq!(cold.label.as_deref(), Some("Ops laptop"));
}

#[test]
fn a_wallet_name_wins_over_the_book() {
    let mut book = AddressBook::new();
    book.insert_account(pk(3), "Old name", Origin::Imported, None);
    let found = directory(
        vec![WalletRecord::offline("Ops laptop", pk(3))],
        book,
        Holdings::default(),
    );
    assert_eq!(found.label_of(&pk(3)).as_deref(), Some("Ops laptop"));
}

#[test]
fn a_squads_wallet_names_its_vault_and_its_multisig() {
    let squad = Squad::v4(&pk(9));
    let wallet = WalletRecord::squads("Treasury", squad, vec![pk(3)], 1);
    let vault = wallet.address().unwrap();
    let found = directory(
        vec![wallet, WalletRecord::offline("Ops laptop", pk(3))],
        AddressBook::new(),
        Holdings::default(),
    );

    let shown = found.describe(&vault);
    assert_eq!(shown.category, Category::Wallet(WalletSort::SmartContract));
    assert_eq!(shown.label.as_deref(), Some("Treasury"));
    let multisig = found.describe(&squad.address);
    assert_eq!(
        multisig.category,
        Category::Wallet(WalletSort::SmartContract)
    );
    assert_eq!(multisig.label.as_deref(), Some("Treasury multisig"));
    assert!(multisig.facts.contains(&Fact::PartOf {
        wallet: vault,
        role: "multisig"
    }));
    assert!(found
        .describe(&pk(3))
        .facts
        .contains(&Fact::MemberOf(vault)));
    assert!(shown.collides_with.is_empty());
}

#[test]
fn a_programmatic_signer_is_a_smart_wallet_and_its_authority_is_not() {
    let signer = WalletRecord::programmatic("Bot", pk(4), None);
    let address = signer.address().unwrap();
    let found = directory(vec![signer], AddressBook::new(), Holdings::default());
    assert_eq!(
        found.describe(&address).category,
        Category::Wallet(WalletSort::SmartContract)
    );
    assert_eq!(found.describe(&pk(4)).category, Category::Unknown);
}

#[test]
fn a_vector_account_is_a_smart_wallet() {
    let wallet = WalletRecord::vector(
        "Quantum",
        SchemeId::Ed25519,
        vector::PROGRAM_ID,
        vec![5; 32],
        Authority::of(None),
    );
    let address = wallet.address().unwrap();
    let found = directory(vec![wallet], AddressBook::new(), Holdings::default());
    assert_eq!(
        found.describe(&address).category,
        Category::Wallet(WalletSort::SmartContract)
    );
}

#[test]
fn the_book_sorts_what_is_not_a_wallet() {
    let mut book = AddressBook::new();
    book.insert_contact(pk(1), "Alice");
    book.insert_kind(pk(2), "USDC", AddressKind::Mint, Origin::Imported, None);
    book.insert_kind(
        pk(3),
        "Helius",
        AddressKind::Validator,
        Origin::Imported,
        None,
    );
    let found = book_only(book);
    assert_eq!(found.describe(&pk(1)).category, Category::Contact);
    assert_eq!(found.describe(&pk(2)).category, Category::Token);
    assert_eq!(found.describe(&pk(3)).category, Category::Validator);
    assert_eq!(
        found.describe(&crate::ids::SYSTEM_PROGRAM_ID).category,
        Category::Program
    );
    assert_eq!(found.describe(&pk(99)).category, Category::Unknown);
}

#[test]
fn a_blank_label_is_none_and_never_the_address() {
    let mut book = AddressBook::new();
    book.insert_contact(pk(1), "  ");
    let found = book_only(book);
    assert_eq!(found.describe(&pk(1)).label, None);
    assert_eq!(found.describe(&pk(99)).label, None);
}

#[test]
fn a_token_account_takes_its_mints_name() {
    let mut book = AddressBook::new();
    book.insert_kind(pk(4), "USDC", AddressKind::Mint, Origin::Imported, None);
    let mut holdings = Holdings::default();
    holdings.replace_owners(
        &[pk(2)],
        &[DiscoveredTokenAccount {
            pubkey: pk(3),
            owner: pk(2),
            mint: pk(4),
            amount: 10,
            decimals: 6,
        }],
    );
    let found = directory(Vec::new(), book, holdings);
    let account = found.describe(&pk(3));
    assert_eq!(account.category, Category::Token);
    assert_eq!(account.label.as_deref(), Some("USDC"));
    assert!(account.facts.contains(&Fact::OwnedBy(pk(2))));
    // A name taken from the mint does not collide with the mint.
    assert!(account.collides_with.is_empty());
}

#[test]
fn a_nonce_account_is_named_from_its_authority() {
    let mut book = AddressBook::new();
    book.apply_discovered_nonce(crate::chain::DiscoveredNonce {
        pubkey: pk(5),
        authority: pk(3),
        hash: solana_hash::Hash::default(),
    });
    let found = directory(
        vec![WalletRecord::offline("Ops laptop", pk(3))],
        book,
        Holdings::default(),
    );
    let nonce = found.describe(&pk(5));
    assert_eq!(nonce.category, Category::Nonce);
    assert_eq!(nonce.label.as_deref(), Some("Nonce of Ops laptop"));
}

#[test]
fn the_same_label_twice_collides() {
    let mut book = AddressBook::new();
    book.insert_contact(pk(1), "Bob");
    book.insert_contact(pk(2), " bob ");
    book.insert_contact(pk(3), "Carol");
    let found = book_only(book);
    assert_eq!(found.describe(&pk(1)).collides_with, vec![pk(2)]);
    assert_eq!(found.describe(&pk(2)).collides_with, vec![pk(1)]);
    assert!(found.describe(&pk(3)).collides_with.is_empty());
    assert_eq!(found.collisions().len(), 1);
}

#[test]
fn a_wallet_and_a_contact_with_one_name_collide() {
    let mut book = AddressBook::new();
    book.insert_contact(pk(1), "Ops laptop");
    let found = directory(
        vec![WalletRecord::offline("Ops laptop", pk(3))],
        book,
        Holdings::default(),
    );
    assert_eq!(found.describe(&pk(3)).collides_with, vec![pk(1)]);
}

#[test]
fn an_imported_label_carries_its_group() {
    let mut book = AddressBook::new();
    book.insert_contact(pk(1), "Bob");
    let mut other = AddressBook::new();
    other.insert_contact(pk(2), "Bob");
    let file = other.export_json("Team").unwrap();
    book.import_json(&file).unwrap();
    let found = book_only(book);
    let imported = found.describe(&pk(2));
    assert_eq!(imported.group.as_deref(), Some("Team"));
    assert_eq!(imported.full_label().as_deref(), Some("Team > Bob"));
    assert!(imported.collides_with.is_empty());
    assert!(found.describe(&pk(1)).collides_with.is_empty());
}

#[test]
fn an_old_book_reads_owners_and_contacts_as_accounts() {
    let json = format!(
        r#"{{"label":"Old","entries":[
            {{"pubkey":"{}","label":"Me","tags":[],"kind":"owner","origin":"derived"}},
            {{"pubkey":"{}","label":"Alice","tags":[],"kind":"contact","origin":"imported"}}
        ]}}"#,
        pk(1),
        pk(2)
    );
    let file: AddressBookFile = serde_json::from_str(&json).unwrap();
    assert!(file
        .entries
        .iter()
        .all(|entry| entry.kind == AddressKind::Account));
}
