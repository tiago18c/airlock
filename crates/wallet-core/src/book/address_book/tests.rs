use super::*;

fn pk(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

#[test]
fn new_book_seeds_programs() {
    let book = AddressBook::new();
    assert_eq!(
        book.get(&SYSTEM_PROGRAM_ID).unwrap().kind,
        AddressKind::Program
    );
    assert!(book.get(&TOKEN_PROGRAM_ID).is_some());
}

#[test]
fn query_matches_label_tag_and_address() {
    let mut book = AddressBook::new();
    let alice = pk(1);
    book.insert_contact(alice, "Alice");
    book.add_tag(&alice, "ops");
    assert_eq!(book.query("alic").count(), 1);
    assert_eq!(book.query("ops").count(), 1);
    assert_eq!(book.query(&alice.to_string()[..8]).count(), 1);
    assert_eq!(book.query("zzz").count(), 0);
}

#[test]
fn clearing_chain_state_keeps_the_entries_and_their_labels() {
    let mut book = AddressBook::new();
    book.insert_account(pk(2), "Treasury", Origin::Derived, None);
    book.set_lamports(&pk(2), 5);
    book.clear_chain_state();
    let entry = book.get(&pk(2)).unwrap();
    assert_eq!(entry.label, "Treasury");
    assert_eq!(entry.lamports, None);
}

#[test]
fn discovered_token_stays_out_of_the_book_unless_already_labeled() {
    let mut book = AddressBook::new();
    let owner = pk(2);
    let ata = pk(3);
    let mint = pk(4);
    book.insert_account(owner, "Treasury", Origin::Derived, None);
    book.apply_discovered_token(DiscoveredTokenAccount {
        pubkey: ata,
        owner,
        mint,
        amount: 10,
        decimals: 6,
    });
    assert!(book.get(&ata).is_none());

    book.insert_contact(ata, "USDC");
    book.apply_discovered_token(DiscoveredTokenAccount {
        pubkey: ata,
        owner,
        mint,
        amount: 11,
        decimals: 6,
    });
    let child = book.get(&ata).unwrap();
    assert_eq!(child.label, "USDC");
    assert_eq!(child.kind, AddressKind::TokenAccount);
    assert_eq!(child.token_amount, Some(11));
    assert_eq!(book.children_of(&owner).count(), 1);
}

#[test]
fn import_keeps_the_source_group() {
    let mut source = AddressBook::new();
    let alice = pk(5);
    source.insert_contact(alice, "Alice");
    source.set_lamports(&alice, 42);
    let json = source.export_json("Laptop").unwrap();
    assert!(!json.contains("System Program"));

    let mut book = AddressBook::new();
    book.insert_contact(alice, "Alice here");
    let label = book.import_json(&json).unwrap();
    assert_eq!(label, "Laptop");
    assert_eq!(book.place(&alice).unwrap().entry.label, "Alice here");
    assert!(!book.place(&alice).unwrap().imported);

    let rows = book.rows("", None);
    let imported = rows
        .iter()
        .find(|row| row.imported && row.entry.pubkey == alice)
        .unwrap();
    assert_eq!(imported.group_label, "Laptop");
    assert_eq!(imported.entry.label, "Alice");
    assert_eq!(imported.entry.lamports, Some(42));
    assert!(rows.iter().any(|row| {
        !row.imported && row.entry.pubkey == alice && row.entry.label == "Alice here"
    }));
    assert!(!book.remove_group("local"));
    assert!(book.remove_group(&imported.group_id));
    assert_eq!(book.rows("", Some(AddressKind::Account)).len(), 1);
    assert!(!book.rename_group(LOCAL_GROUP_ID, "Renamed"));
}

#[test]
fn rename_changes_only_the_imported_group() {
    let mut book = AddressBook::new();
    let alice = pk(7);
    book.insert_contact(alice, "Alice");
    let json = book.export_json("Laptop").unwrap();
    book.import_json(&json).unwrap();
    let imported = book
        .groups()
        .into_iter()
        .find(|group| group.imported)
        .unwrap();
    assert!(book.rename_group(&imported.id, "Office"));
    let place = book.rows_in(&imported.id, "", None);
    assert_eq!(place[0].group_label, "Office");
    assert!(book.place(&alice).is_some_and(|found| !found.imported));
}
