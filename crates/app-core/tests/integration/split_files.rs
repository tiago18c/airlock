//! The workspace in its three files, and the record of what keys have signed with,
//! as the core keeps them.

use app_core::storage::{CHAIN_CACHE_FILE, GRANTS_FILE, LEAF_MARKS_FILE, WORKSPACE_FILE};
use app_core::testing::{kit, kit_on, settle, Kit, MemoryStorage};
use app_core::SAVE_INTERVAL;
use solana_pubkey::Pubkey;

fn pk(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

fn text(kit: &Kit, name: &str) -> String {
    String::from_utf8(kit.storage.read(name).unwrap_or_default()).unwrap()
}

/// Let a deferred save happen.
async fn saved(kit: &Kit) {
    settle().await;
    kit.clock.advance(SAVE_INTERVAL);
    settle().await;
}

#[tokio::test]
async fn a_balance_writes_only_what_was_read_and_a_name_or_a_grant_writes_only_its_own() {
    let kit = kit();
    let public = kit.core.public();
    public.workspace_deferred("add", |ws| {
        ws.book.insert_contact(pk(1), "Alice");
    });
    saved(&kit).await;
    let (user, grants) = (
        kit.storage.writes(WORKSPACE_FILE),
        kit.storage.writes(GRANTS_FILE),
    );
    assert!(user > 0);

    public.workspace_deferred("balance", |ws| {
        ws.book.set_lamports(&pk(1), 7);
    });
    saved(&kit).await;
    assert_eq!(kit.storage.writes(WORKSPACE_FILE), user);
    assert_eq!(kit.storage.writes(GRANTS_FILE), grants);
    assert!(text(&kit, CHAIN_CACHE_FILE).contains("lamports"));
    assert!(!text(&kit, WORKSPACE_FILE).contains("lamports"));

    let chain = kit.storage.writes(CHAIN_CACHE_FILE);
    public.workspace_deferred("rename", |ws| {
        ws.book.set_label(&pk(1), "Alice B");
    });
    saved(&kit).await;
    assert_eq!(kit.storage.writes(WORKSPACE_FILE), user + 1);
    assert_eq!(kit.storage.writes(GRANTS_FILE), grants);
    assert_eq!(kit.storage.writes(CHAIN_CACHE_FILE), chain);
    assert!(text(&kit, WORKSPACE_FILE).contains("Alice B"));
}

#[tokio::test]
async fn a_restart_finds_it_all_together() {
    let kit_before = kit();
    kit_before.core.public().workspace_deferred("add", |ws| {
        ws.book.insert_contact(pk(1), "Alice");
        ws.book.set_lamports(&pk(1), 7);
        ws.connections.remember("https://site.example", vec![pk(9)]);
    });
    saved(&kit_before).await;
    assert!(text(&kit_before, GRANTS_FILE).contains("site.example"));
    assert!(!text(&kit_before, WORKSPACE_FILE).contains("site.example"));

    let again = kit_on(kit_before.storage.clone());
    let book = again.core.slices().book.get();
    let entry = book.get(&pk(1)).unwrap();
    assert_eq!((entry.label.as_str(), entry.lamports), ("Alice", Some(7)));
    assert!(again
        .core
        .with_workspace(|ws| ws.connections.grant("https://site.example").is_some()));
    assert!(again.core.load_problems().is_empty());
}

#[tokio::test]
async fn damaged_grants_are_said_and_sites_ask_again_and_the_book_is_all_there() {
    let before = kit();
    before.core.public().workspace_deferred("add", |ws| {
        ws.book.insert_contact(pk(1), "Alice");
        ws.connections.remember("https://site.example", vec![pk(9)]);
    });
    saved(&before).await;
    before.storage.put(GRANTS_FILE, b"not json");

    let after = kit_on(before.storage.clone());
    assert!(after
        .core
        .with_workspace(|ws| ws.connections.grant("https://site.example").is_none()));
    assert_eq!(
        after.core.slices().book.get().get(&pk(1)).unwrap().label,
        "Alice"
    );
    let problems = after.core.load_problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("list of connected sites"),
        "{problems:?}"
    );
}

#[tokio::test]
async fn damage_to_what_was_read_is_not_worth_saying() {
    let before = kit();
    before.core.public().workspace_deferred("add", |ws| {
        ws.book.insert_contact(pk(1), "Alice");
        ws.book.set_lamports(&pk(1), 7);
    });
    saved(&before).await;
    before.storage.put(CHAIN_CACHE_FILE, b"not json");

    let after = kit_on(before.storage.clone());
    let book = after.core.slices().book.get();
    let entry = book.get(&pk(1)).unwrap();
    assert_eq!((entry.label.as_str(), entry.lamports), ("Alice", None));
    assert!(after.core.load_problems().is_empty());
}

#[tokio::test]
async fn the_record_of_what_keys_signed_is_the_cores_and_damage_to_it_is_said() {
    let storage = MemoryStorage::default();
    storage.put(LEAF_MARKS_FILE, b"not json");
    let kit = kit_on(storage);
    let problems = kit.core.load_problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("used signing leaves"), "{problems:?}");
    // It fails closed, and what it holds is left as it is.
    assert_eq!(kit.storage.read(LEAF_MARKS_FILE).unwrap(), b"not json");
}

#[tokio::test]
async fn a_rotation_that_may_not_have_landed_reaches_the_screens_through_the_wallets() {
    let storage = MemoryStorage::default();
    let next_key = vec![9u8; 41];
    let marks = serde_json::json!({
        "rotations": { pk(1).to_string(): { "nextKey": hex::encode(&next_key), "digests": [] } }
    });
    storage.put(LEAF_MARKS_FILE, marks.to_string().as_bytes());
    let kit = kit_on(storage);
    // Nothing shows before there is a vault.
    assert!(kit.core.slices().wallets.get().pending_rotations.is_empty());

    kit.core.create_vault("pw").unwrap();
    assert_eq!(
        kit.core.slices().wallets.get().pending_rotations,
        [(pk(1), next_key.clone())]
    );

    // Whoever takes the vault with the marks reads the same record.
    let read = kit
        .core
        .with_vault_and_marks(|_, marks| marks.pending_rotation(&pk(1)));
    assert_eq!(read, Some(next_key));
}
