//! Each row of "Airlock's profiles".

use super::*;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::wallets::hardware::HardwareVendor;
use crate::wallets::programmatic::ED25519_SIGNER_PROGRAM_ID;
use crate::wallets::record::{Authority, WalletRecord};
use crate::wallets::squads::{
    Squad, SquadState, SquadsMember, SquadsProgram, PERMISSION_ALL, PERMISSION_VOTE,
};
use crate::wallets::vector::SchemeId;

fn hot(label: &str) -> (WalletRecord, Pubkey) {
    let key = KeypairSigner::new_random();
    (
        WalletRecord::from_keypair_seed(label, key.seed_bytes()),
        key.pubkey(),
    )
}

fn profile(wallets: &[WalletRecord], account: Pubkey) -> SigningProfile {
    signing_profile(wallets, account, None).unwrap()
}

#[test]
fn a_hot_key_or_a_hardware_wallet_does_everything_at_once() {
    let (wallet, key) = hot("Hot");
    let hardware =
        WalletRecord::hardware("Ledger", HardwareVendor::Ledger, pk(5), "m/44'/501'/0'/0'");
    let wallets = vec![wallet, hardware];
    for account in [key, pk(5)] {
        let profile = profile(&wallets, account);
        assert_eq!(profile.timing, Timing::Sync);
        assert_eq!(profile.execution, Execution::TopLevel);
        assert_eq!(profile.carrier, None);
        assert_eq!(
            profile.fee_payer,
            vec![Payer::Account, Payer::Wallet, Payer::App]
        );
        assert_eq!(
            profile.new_accounts,
            vec![NewAccountKey::WalletKey, NewAccountKey::AppKey]
        );
        assert_eq!(
            profile.delivery,
            vec![Delivery::Send, Delivery::Cosign, Delivery::Return]
        );
        assert_eq!(
            profile.messages,
            vec![MessageMode::Signature, MessageMode::Memo]
        );
        assert_eq!(profile.max_transactions, 16);
    }
}

#[test]
fn a_profile_is_written_as_the_reference_writes_it() {
    let (wallet, key) = hot("Hot");
    let json = serde_json::to_value(profile(&[wallet], key)).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "account": key.to_string(),
            "timing": "sync",
            "execution": "top-level",
            "feePayer": ["account", "wallet", "app"],
            "newAccounts": ["wallet-key", "app-key"],
            "delivery": ["send", "cosign", "return"],
            "messages": ["signature", "memo"],
            "maxTransactions": 16,
        })
    );
}

#[test]
fn an_offline_key_signs_later_at_the_top() {
    let offline = WalletRecord::offline("Cold", pk(6));
    let profile = profile(&[offline], pk(6));
    assert_eq!(profile.timing, Timing::Async);
    assert_eq!(profile.execution, Execution::TopLevel);
    assert_eq!(profile.fee_payer, vec![Payer::Account, Payer::Wallet]);
    assert_eq!(profile.new_accounts, vec![NewAccountKey::WalletKey]);
    assert_eq!(profile.delivery, vec![Delivery::Send]);
    assert_eq!(profile.messages, vec![MessageMode::Memo]);
}

fn assert_carrier(profile: &SigningProfile, timing: Timing, carrier: Pubkey) {
    assert_eq!(profile.timing, timing);
    assert_eq!(profile.execution, Execution::Carrier);
    assert_eq!(profile.carrier, Some(carrier));
    assert_eq!(profile.fee_payer, vec![Payer::Wallet]);
    // A multisig's program signs for a new account at an ephemeral signer,
    // at once or later. Anything else signs with a key made here.
    let new_accounts = if SquadsProgram::from_program_id(&carrier).is_some() {
        vec![NewAccountKey::Carrier]
    } else {
        vec![NewAccountKey::WalletKey]
    };
    assert_eq!(profile.new_accounts, new_accounts);
    assert_eq!(profile.delivery, vec![Delivery::Send]);
    assert_eq!(profile.messages, vec![MessageMode::Memo]);
    // An authorization's nonce moves with each transaction that runs.
    let ordered = SquadsProgram::from_program_id(&carrier).is_none();
    assert_eq!(profile.max_transactions, if ordered { 1 } else { 16 });
}

#[test]
fn a_programmatic_signer_is_sync_when_its_authority_is_here() {
    let (key, authority) = hot("Authority");
    let here = WalletRecord::programmatic("Here", authority, Some(key.id));
    let offline = WalletRecord::offline("Cold", pk(6));
    let elsewhere = WalletRecord::programmatic("Elsewhere", pk(6), Some(offline.id));
    let outside = WalletRecord::programmatic("Outside", pk(7), None);
    let wallets = vec![
        key,
        here.clone(),
        offline,
        elsewhere.clone(),
        outside.clone(),
    ];
    for (wallet, timing) in [
        (&here, Timing::Sync),
        (&elsewhere, Timing::Async),
        (&outside, Timing::Async),
    ] {
        let profile = profile(&wallets, wallet.address().unwrap());
        assert_carrier(&profile, timing, ED25519_SIGNER_PROGRAM_ID);
    }
}

#[test]
fn a_vector_wallet_is_sync_when_its_key_is_here() {
    let vector = |label: &str, scheme: SchemeId, authority: Authority| {
        WalletRecord::vector(label, scheme, pk(70), vec![7; scheme.key_len()], authority)
    };
    let held = vector(
        "Held",
        SchemeId::Ed25519,
        Authority::Held {
            secret: wallet_vault::SecretBytes(vec![1; 32]),
            next_leaf: 0,
        },
    );
    let outside = vector("Outside", SchemeId::Eip191, Authority::Outside);
    let wallets = vec![held.clone(), outside.clone()];
    assert_carrier(
        &profile(&wallets, held.address().unwrap()),
        Timing::Sync,
        pk(70),
    );
    assert_carrier(
        &profile(&wallets, outside.address().unwrap()),
        Timing::Async,
        pk(70),
    );
}

/// A multisig of `members` at `threshold`, acted on by `acting`, with
/// `with` approving together with it.
fn multisig(
    program: SquadsProgram,
    members: Vec<Pubkey>,
    threshold: u16,
    acting: &WalletRecord,
    with: &[&WalletRecord],
) -> WalletRecord {
    let squad = match program {
        SquadsProgram::V4 => Squad::v4(&pk(90)),
        SquadsProgram::SmartAccount => Squad::smart_account(9),
    };
    let mut wallet = WalletRecord::squads("Treasury", squad, members, threshold);
    wallet.kind.set_member_wallet(Some(acting.id));
    wallet
        .kind
        .set_approves_with(with.iter().map(|other| other.id).collect());
    wallet
}

#[test]
fn a_multisig_this_device_carries_alone_is_sync() {
    let (first, one) = hot("One");
    let (second, two) = hot("Two");
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        // One vote, and the acting member casts it.
        let alone = multisig(program, vec![one, pk(8)], 1, &first, &[]);
        let wallets = vec![first.clone(), second.clone(), alone.clone()];
        assert_carrier(
            &profile(&wallets, alone.address().unwrap()),
            Timing::Sync,
            program.program_id(),
        );

        // Two votes, both here.
        let together = multisig(program, vec![one, two, pk(8)], 2, &first, &[&second]);
        let wallets = vec![first.clone(), second.clone(), together.clone()];
        assert_carrier(
            &profile(&wallets, together.address().unwrap()),
            Timing::Sync,
            program.program_id(),
        );
    }
}

#[test]
fn any_other_multisig_is_async() {
    let (first, one) = hot("One");
    let offline = WalletRecord::offline("Cold", pk(6));
    let program = SquadsProgram::V4;
    fn async_profile(wallets: &[WalletRecord], squad: &WalletRecord, state: Option<&SquadState>) {
        let profile = signing_profile(wallets, squad.address().unwrap(), state).unwrap();
        assert_carrier(&profile, Timing::Async, SquadsProgram::V4.program_id());
    }

    // Two votes, one here.
    let short = multisig(program, vec![one, pk(8)], 2, &first, &[]);
    async_profile(&[first.clone(), short.clone()], &short, None);

    // Two votes, the second on another device.
    let elsewhere = multisig(program, vec![one, pk(6)], 2, &first, &[&offline]);
    async_profile(
        &[first.clone(), offline.clone(), elsewhere.clone()],
        &elsewhere,
        None,
    );

    // The threshold was never read.
    let unknown = multisig(program, Vec::new(), 0, &first, &[]);
    async_profile(&[first.clone(), unknown.clone()], &unknown, None);

    // The acting member's key is on another device, whatever the threshold.
    let cold = multisig(program, vec![pk(6)], 1, &offline, &[]);
    async_profile(&[offline.clone(), cold.clone()], &cold, None);

    // What the chain says: a time lock, or no member here that may execute.
    let alone = multisig(program, vec![one], 1, &first, &[]);
    let state = |permissions, time_lock| SquadState {
        threshold: 1,
        time_lock,
        transaction_index: 0,
        stale_transaction_index: 0,
        members: vec![SquadsMember {
            key: one,
            permissions,
        }],
    };
    let wallets = [first.clone(), alone.clone()];
    async_profile(&wallets, &alone, Some(&state(PERMISSION_ALL, 60)));
    async_profile(&wallets, &alone, Some(&state(PERMISSION_VOTE, 0)));
    let sync = signing_profile(
        &wallets,
        alone.address().unwrap(),
        Some(&state(PERMISSION_ALL, 0)),
    );
    assert_eq!(sync.unwrap().timing, Timing::Sync);
}

#[test]
fn a_multisig_lists_its_carrier_for_new_accounts() {
    let (first, one) = hot("One");
    for (threshold, timing) in [(1, "sync"), (2, "async")] {
        let squad = multisig(SquadsProgram::V4, vec![one, pk(8)], threshold, &first, &[]);
        let wallets = [first.clone(), squad.clone()];
        let json = serde_json::to_value(profile(&wallets, squad.address().unwrap())).unwrap();
        assert_eq!(json["timing"], timing);
        assert_eq!(json["newAccounts"], serde_json::json!(["carrier"]));
    }
}

#[test]
fn an_account_that_is_no_wallet_here_has_no_profile() {
    let (wallet, _) = hot("Hot");
    assert_eq!(
        signing_profile(&[wallet], pk(99), None),
        Err(ProfileError::NotAWallet(pk(99)))
    );
}
