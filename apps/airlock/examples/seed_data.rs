//! Write a fixed vault, workspace, and holdings file for smoke runs.
//!
//! ```text
//! cargo run --example seed_data -- --dir <path>
//!
//! PowerShell:
//!   $env:AIRLOCK_DATA_DIR="<path>"; & dx serve -p airlock --desktop
//! cmd.exe / Unix:
//!   set AIRLOCK_DATA_DIR=<path>   &   export AIRLOCK_DATA_DIR=<path>
//! ```

use std::path::{Path, PathBuf};
use std::process;

use solana_hash::Hash;
use solana_pubkey::Pubkey;
use wallet_core::vector::{SchemeId, PROGRAM_ID};
use wallet_core::{
    derive_batch, name_offline_wallet, name_signers, Authority, Cluster, DiscoveredNonce,
    DiscoveredTokenAccount, FileStore, HoldingsStore, KeypairSigner, NewSigner, SecretBytes, Vault,
    WalletClass, WalletRecord, WalletSigner, WorkspaceStore, SOLANA_PATH_TEMPLATE,
};

/// Unlock password for the fixture vault. Never logged by the app.
const PASSWORD: &str = "fixture-password";

/// Fixed 24-word phrase. Hot wallets are derived from it; never logged.
const SEED: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

fn main() {
    let dir = match parse_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("{err}");
            process::exit(2);
        }
    };
    if let Err(err) = write_fixture(&dir) {
        eprintln!("seed_data: {err}");
        process::exit(1);
    }
    println!("Wrote fixture to {}", dir.display());
    println!("Password: {PASSWORD}");
    println!(
        "Start the app with AIRLOCK_DATA_DIR={0} (Airlock Offline: AIRLOCK_OFFLINE_DATA_DIR={0})",
        dir.display()
    );
}

fn parse_dir() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--dir"), Some(path)) => Ok(PathBuf::from(path)),
        _ => Err("usage: cargo run --example seed_data -- --dir <path>".into()),
    }
}

fn write_fixture(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let accounts = derive_batch(SEED, "", SOLANA_PATH_TEMPLATE, 0, 5).map_err(|e| e.to_string())?;
    let hot = NewSigner::from_seed_accounts("Hot", SEED, "", &accounts);

    let mut vault: Vault<_> = Vault::new(FileStore::new(dir.join("vault.json")));
    vault.create(PASSWORD).map_err(|e| e.to_string())?;
    vault
        .change(|data| {
            data.add_signers(&hot).expect("hot wallets");
            for i in 0u8..5 {
                let key = KeypairSigner::from_seed(&fixed_seed(100 + i)).expect("offline key");
                data.add_offline_wallet(format!("Offline {i}"), key.pubkey(), None)
                    .expect("offline wallet");
            }
            for i in 0u8..5 {
                let authority = KeypairSigner::from_seed(&fixed_seed(110 + i))
                    .expect("programmatic authority")
                    .pubkey();
                let wallet =
                    WalletRecord::programmatic(format!("Programmatic {i}"), authority, None);
                data.add_contract_wallet(wallet).expect("programmatic");
            }
            for i in 0u8..5 {
                let key = KeypairSigner::from_seed(&fixed_seed(120 + i)).expect("vector key");
                let wallet = WalletRecord::vector(
                    format!("Vector {i}"),
                    SchemeId::Ed25519,
                    PROGRAM_ID,
                    key.pubkey().to_bytes().to_vec(),
                    Authority::Held {
                        secret: SecretBytes(key.seed_bytes().to_vec()),
                        next_leaf: 0,
                    },
                );
                data.add_contract_wallet(wallet).expect("vector");
            }
            data.selected = data.wallets.first().map(|w| w.id);
        })
        .map_err(|e| e.to_string())?;

    let contract_names: Vec<(Pubkey, String)> = Vault::data(&vault)
        .map_err(|e| e.to_string())?
        .wallets
        .iter()
        .filter(|wallet| wallet.kind.class() == WalletClass::SmartContract)
        .filter_map(|wallet| Some((wallet.address()?, wallet.label.clone())))
        .collect();

    let mut workspace = WorkspaceStore::open(FileStore::new(dir.join("workspace.json")));
    workspace
        .change(|ws| {
            ws.cluster = Cluster::Devnet;
            ws.vector_wallets = true;
            name_signers(&mut ws.book, &hot);
            for i in 0u8..5 {
                let key = KeypairSigner::from_seed(&fixed_seed(100 + i)).expect("offline key");
                name_offline_wallet(&mut ws.book, key.pubkey(), format!("Offline {i}"));
            }
            for (address, label) in &contract_names {
                ws.book.insert_contact(*address, label.clone());
            }
            for i in 0u16..200 {
                ws.book
                    .insert_contact(contact_pubkey(i), format!("Contact {i:03}"));
            }
            // One durable nonce owned by the first hot wallet, for Build → Sign.
            ws.book.apply_discovered_nonce(DiscoveredNonce {
                pubkey: nonce_pubkey(),
                authority: hot[0].pubkey,
                hash: Hash::new_from_array([5; 32]),
            });
        })
        .map_err(|e| e.to_string())?;

    let first_owner = hot[0].pubkey;
    let mut holdings = HoldingsStore::open(FileStore::new(dir.join("holdings.json")));
    holdings
        .change(|h| {
            h.replace_owners(
                &[first_owner],
                &[
                    DiscoveredTokenAccount {
                        pubkey: Pubkey::new_from_array([9; 32]),
                        owner: first_owner,
                        mint: Pubkey::new_from_array([8; 32]),
                        amount: 1_000_000,
                        decimals: 6,
                    },
                    DiscoveredTokenAccount {
                        pubkey: Pubkey::new_from_array([7; 32]),
                        owner: first_owner,
                        mint: Pubkey::new_from_array([6; 32]),
                        amount: 42,
                        decimals: 0,
                    },
                ],
            );
        })
        .map_err(|e| e.to_string())?;

    let marks = dir.join("vector-leaves.json");
    if !marks.exists() {
        std::fs::write(&marks, b"{}").map_err(|e| e.to_string())?;
    }

    verify_fixture(dir)?;
    Ok(())
}

/// Reload what was written and refuse to leave a half-built fixture.
fn verify_fixture(dir: &Path) -> Result<(), String> {
    let mut vault: Vault<_> = Vault::new(FileStore::new(dir.join("vault.json")));
    vault.unlock(PASSWORD).map_err(|e| e.to_string())?;
    let data = Vault::data(&vault).map_err(|e| e.to_string())?;
    if data.wallets.len() != 20 {
        return Err(format!("expected 20 wallets, got {}", data.wallets.len()));
    }
    let hot = data
        .wallets
        .iter()
        .filter(|w| w.kind.class() == WalletClass::Hot)
        .count();
    let offline = data
        .wallets
        .iter()
        .filter(|w| w.kind.class() == WalletClass::Offline)
        .count();
    let contracts = data
        .wallets
        .iter()
        .filter(|w| w.kind.class() == WalletClass::SmartContract)
        .count();
    if (hot, offline, contracts) != (5, 5, 10) {
        return Err(format!(
            "expected 5 hot / 5 offline / 10 contract, got {hot}/{offline}/{contracts}"
        ));
    }
    let workspace = WorkspaceStore::open(FileStore::new(dir.join("workspace.json")));
    let contacts = workspace
        .data()
        .book
        .iter()
        .filter(|e| e.kind == wallet_core::AddressKind::Account)
        .count();
    if contacts < 200 {
        return Err(format!("expected at least 200 contacts, got {contacts}"));
    }
    let nonces = workspace
        .data()
        .book
        .iter()
        .filter(|e| e.kind == wallet_core::AddressKind::NonceAccount)
        .count();
    if nonces == 0 {
        return Err("expected a durable nonce in the book".into());
    }
    if !workspace.data().vector_wallets {
        return Err("vector wallets should be offered".into());
    }
    let holdings = HoldingsStore::open(FileStore::new(dir.join("holdings.json")));
    if holdings
        .data()
        .summary(&data.wallets[0].address().unwrap())
        .total
        == 0
    {
        return Err("expected holdings for the first hot wallet".into());
    }
    Ok(())
}

fn fixed_seed(n: u8) -> [u8; 32] {
    let mut seed = [0u8; 32];
    seed[0] = n;
    seed[31] = n.wrapping_add(1);
    seed
}

fn contact_pubkey(i: u16) -> Pubkey {
    let mut bytes = [0u8; 32];
    bytes[0] = 0xC0;
    bytes[1] = (i >> 8) as u8;
    bytes[2] = i as u8;
    Pubkey::new_from_array(bytes)
}

fn nonce_pubkey() -> Pubkey {
    Pubkey::new_from_array([0xD1; 32])
}
