//! Tests of the wallet record and how it is stored.

use super::record::*;
use super::squads::{Squad, SquadsProgram};
use super::vector::SchemeId;
use crate::ids::WalletId;
use solana_pubkey::Pubkey;
use wallet_vault::SecretBytes;

#[test]
fn debug_does_not_print_secrets() {
    let wallet = WalletRecord::from_keypair_seed("hot", [0xab; 32]);
    let shown = format!("{wallet:?}");
    assert!(shown.contains("redacted"));
    assert!(!shown.contains("abab"));
}

#[test]
fn a_programmatic_signer_of_either_protocol_survives_being_stored() {
    let key = Pubkey::new_from_array([1; 32]);
    let hot = WalletId::new();
    let mut signer = WalletRecord::programmatic("Signer", key, Some(hot));
    signer
        .kind
        .set_executor_nonce(Some(Pubkey::new_from_array([7; 32])));
    let vector = WalletRecord::vector(
        "Quantum",
        SchemeId::MlDsa44,
        Pubkey::new_from_array([2; 32]),
        vec![9; 1312],
        Authority::Held {
            secret: SecretBytes(vec![0xab; 32]),
            next_leaf: 0,
        },
    );
    for wallet in [signer, vector] {
        let saved = serde_json::to_value(&wallet).unwrap();
        assert_eq!(saved["kind"]["type"], "programmatic");
        let loaded: WalletRecord = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded, wallet);
    }
}

#[test]
fn a_key_kept_in_a_record_is_not_printed_and_a_long_public_key_is_not_dumped() {
    let vector = WalletRecord::vector(
        "Quantum",
        SchemeId::MlDsa44,
        Pubkey::new_from_array([2; 32]),
        vec![9; 1312],
        Authority::Held {
            secret: SecretBytes(vec![0xab; 32]),
            next_leaf: 0,
        },
    );
    let shown = format!("{vector:?}");
    assert!(shown.contains("redacted") && !shown.contains("abab"));
    assert!(shown.contains("PublicBytes(1312 bytes)"));
    assert_eq!(vector.summary(), "Vector wallet · ML-DSA-44");
    assert_eq!(vector.kind.class(), WalletClass::SmartContract);
    assert!(!vector.kind.can_sign());
}

fn multisig() -> WalletRecord {
    let squad = Squad {
        program: SquadsProgram::V4,
        address: Pubkey::new_from_array([4; 32]),
    };
    WalletRecord::squads("Team", squad, Vec::new(), 2)
}

fn approvers(wallet: &WalletRecord) -> (Option<WalletId>, Vec<WalletId>, Option<Pubkey>) {
    match &wallet.kind {
        WalletKind::Squads {
            member_wallet,
            approves_with,
            approval_nonce,
            ..
        } => (*member_wallet, approves_with.clone(), *approval_nonce),
        _ => panic!("a multisig"),
    }
}

#[test]
fn a_multisig_saved_before_combined_approvals_loads_with_none_and_writes_none() {
    let wallet = multisig();
    let saved = serde_json::to_value(&wallet).unwrap();
    assert!(saved["kind"].get("approvesWith").is_none());
    assert!(saved["kind"].get("approvalNonce").is_none());
    // What an earlier version wrote has neither field.
    let loaded: WalletRecord = serde_json::from_value(saved).unwrap();
    assert_eq!(approvers(&loaded), (None, Vec::new(), None));

    let mut set = wallet;
    let other = WalletId::new();
    let nonce = Pubkey::new_from_array([7; 32]);
    set.kind.set_approves_with(vec![other]);
    set.kind.set_approval_nonce(Some(nonce));
    let again: WalletRecord = serde_json::from_value(serde_json::to_value(&set).unwrap()).unwrap();
    assert_eq!(approvers(&again), (None, vec![other], Some(nonce)));
}

#[test]
fn the_wallets_that_approve_together_are_listed_once_and_never_the_acting_member() {
    let mut wallet = multisig();
    let (acting, a, b) = (WalletId::new(), WalletId::new(), WalletId::new());
    wallet.kind.set_member_wallet(Some(acting));
    wallet.kind.set_approves_with(vec![b, acting, a, b]);
    assert_eq!(approvers(&wallet).1, vec![b, a]);

    // A listed wallet that becomes the acting member leaves the list.
    wallet.kind.set_member_wallet(Some(a));
    assert_eq!(approvers(&wallet), (Some(a), vec![b], None));

    // Neither is the executor nonce a multisig's to have.
    let mut plain = multisig();
    plain
        .kind
        .set_executor_nonce(Some(Pubkey::new_from_array([7; 32])));
    assert_eq!(plain.kind.executor_nonce(), None);

    // These settings belong to a multisig only.
    let mut signer = WalletRecord::programmatic("Signer", Pubkey::new_from_array([1; 32]), None);
    let before = signer.clone();
    signer.kind.set_approves_with(vec![a]);
    signer.kind.set_member_wallet(Some(a));
    signer
        .kind
        .set_approval_nonce(Some(Pubkey::new_from_array([7; 32])));
    assert_eq!(signer, before);
}

#[test]
fn an_offline_wallet_keeps_its_nonce_and_path_and_one_from_before_still_loads() {
    let mut cold = WalletRecord::offline("Cold", Pubkey::new_from_array([1; 32]));
    let before = serde_json::to_value(&cold).unwrap();
    assert!(before["kind"].get("durable_nonce").is_none());
    assert!(before["kind"].get("default_path").is_none());
    let loaded: WalletRecord = serde_json::from_value(before).unwrap();
    assert_eq!(loaded, cold);

    cold.kind
        .set_durable_nonce(Some(Pubkey::new_from_array([2; 32])));
    cold.kind.set_default_path(Some(OfflinePath::DurableNonce));
    let saved = serde_json::to_value(&cold).unwrap();
    assert_eq!(saved["kind"]["default_path"], "durableNonce");
    let loaded: WalletRecord = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded, cold);
}

#[test]
fn a_phrase_wallet_without_secrets_keeps_its_key_number_and_not_its_words() {
    let phrase = "legal winner thank year wave sausage worth useful legal winner thank yellow";
    let wallet = WalletRecord::vector_from_phrase(
        "Tree",
        SchemeId::Xmss,
        super::vector::PROGRAM_ID,
        phrase,
        true,
    )
    .unwrap();
    assert_eq!(wallet.vector_phrase().unwrap().words.0, phrase);
    let listed = wallet.without_secrets();
    let kept = listed.vector_phrase().unwrap();
    assert!(kept.words.0.is_empty());
    assert_eq!((kept.index, kept.restored), (0, true));
    assert!(!format!("{wallet:?}").contains("legal"));
    // Stored and read back whole.
    let json = serde_json::to_string(&wallet).unwrap();
    let back: WalletRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back, wallet);
}
