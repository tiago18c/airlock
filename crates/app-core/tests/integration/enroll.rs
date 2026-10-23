//! `Core::enroll` stores a wallet and names it in the book as one command, and
//! publishes both slices. Phase 1.3's `Session` goes through it.

use app_core::testing::kit;
use solana_pubkey::Pubkey;

fn pk(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

#[tokio::test]
async fn enroll_publishes_wallets_and_book() {
    let kit = kit();
    kit.core.create_vault("pw").unwrap();
    let wallets = kit.core.slices().wallets.subscribe();
    let book = kit.core.slices().book.subscribe();

    let id = kit
        .core
        .enroll(
            "add_offline_wallet",
            |data| data.add_offline_wallet("cold", pk(7), None),
            |book, _| {
                book.insert_contact(pk(7), "cold".to_string());
            },
        )
        .unwrap();

    assert!(wallets.has_changed().unwrap());
    assert!(book.has_changed().unwrap());
    let view = kit.core.slices().wallets.get();
    assert!(view.summaries.iter().any(|summary| summary.id == id));
    assert!(kit.core.slices().book.get().get(&pk(7)).is_some());
}

#[tokio::test]
async fn enroll_on_a_locked_vault_changes_nothing() {
    let kit = kit();
    let book = kit.core.slices().book.subscribe();

    let result = kit.core.enroll(
        "add_offline_wallet",
        |data| data.add_offline_wallet("cold", pk(7), None),
        |book, _| {
            book.insert_contact(pk(7), "cold".to_string());
        },
    );

    assert!(result.is_err());
    assert!(!book.has_changed().unwrap());
    assert!(kit.core.slices().book.get().get(&pk(7)).is_none());
}

#[tokio::test]
async fn with_workspace_reads_what_the_slices_leave_out() {
    let kit = kit();
    kit.core
        .workspace_checked("custom", |ws| {
            ws.custom_cluster = Some(wallet_core::Cluster::Testnet);
        })
        .unwrap();
    let custom = kit.core.with_workspace(|ws| ws.custom_cluster.clone());
    assert_eq!(custom, Some(wallet_core::Cluster::Testnet));
}
