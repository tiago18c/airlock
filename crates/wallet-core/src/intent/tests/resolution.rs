//! A resolution's states, each change from where it may start.

use super::*;
use crate::fixtures::transfer;
use crate::keys::signer::{KeypairSigner, WalletSigner};
use crate::tx::decode::parse_wire;
use crate::tx::signatures::apply_local_signatures;

const NOW: u64 = 1_800_000_000;

fn fresh(delivery: Delivery, requested: usize) -> Resolution {
    Resolution::new(
        "mint-1",
        pk(2),
        "solana:devnet",
        Timing::Sync,
        delivery,
        requested,
        NOW,
    )
}

/// A transaction `wallet` pays for that `app` also signs, signed by the
/// wallet only.
fn cosigned(wallet: &KeypairSigner, app: &KeypairSigner) -> Vec<u8> {
    let mut ix = transfer(wallet.pubkey(), pk(9), 5);
    ix.accounts
        .push(AccountMeta::new_readonly(app.pubkey(), true));
    let wire = compile_v1(wallet.pubkey(), vec![ix], None, Some(blockhash(1)))
        .unwrap()
        .wire;
    apply_local_signatures(&wire, &[wallet as &dyn WalletSigner]).unwrap()
}

fn composed(wire: Vec<u8>) -> Vec<Composed> {
    vec![Composed {
        role: TxRole::Direct,
        of: vec![0],
        wire,
    }]
}

#[test]
fn a_new_resolution_awaits_approval() {
    let resolution = fresh(Delivery::Send, 1);
    assert_eq!(resolution.state, State::AwaitingApproval);
    assert_eq!(resolution.updated_at, NOW);
    let json = resolution.public();
    assert_eq!(json["state"], "awaiting-approval");
    assert_eq!(json["delivery"], "send");
    assert_eq!(json["timing"], "sync");
    assert_eq!(json["account"], pk(2).to_string());
    assert_eq!(json["transactions"], serde_json::json!([]));
    assert_eq!(json["ran"], serde_json::json!([]));
    assert_eq!(json["updatedAt"], NOW);
    assert!(json.get("held").is_none());
    assert!(json.get("requested").is_none());
    assert!(json.get("order").is_none());
}

#[test]
fn declined_expired_and_cancelled_only_from_where_they_may_start() {
    for (change, ends) in [
        (
            Resolution::decline as fn(&mut Resolution, u64) -> _,
            State::Declined,
        ),
        (Resolution::expire, State::Expired),
        (Resolution::cancel, State::Cancelled),
    ] {
        let mut resolution = fresh(Delivery::Send, 1);
        change(&mut resolution, NOW + 1).unwrap();
        assert_eq!(resolution.state, ends);
        assert_eq!(resolution.updated_at, NOW + 1);
        let err = change(&mut resolution, NOW + 2).unwrap_err();
        assert_eq!(err.code(), "bad_state");
    }

    // Once signed and sending, nothing of these.
    let wallet = KeypairSigner::new_random();
    let mut sending = fresh(Delivery::Send, 1);
    let wire = apply_local_signatures(
        &compile_v1(
            wallet.pubkey(),
            vec![transfer(wallet.pubkey(), pk(9), 5)],
            None,
            Some(blockhash(1)),
        )
        .unwrap()
        .wire,
        &[&wallet as &dyn WalletSigner],
    )
    .unwrap();
    sending.signed(composed(wire), NOW).unwrap();
    assert_eq!(sending.state, State::Sending);
    assert!(sending.cancel(NOW).is_err());
    assert!(sending.expire(NOW).is_err());
    assert!(sending.decline(NOW).is_err());
}

#[test]
fn cosigned_the_app_gets_empty_slots_and_owes_its_signature() {
    let wallet = KeypairSigner::new_random();
    let app = KeypairSigner::new_random();
    let signed = cosigned(&wallet, &app);
    let mut resolution = fresh(Delivery::Cosign, 1);
    resolution.signed(composed(signed.clone()), NOW).unwrap();

    assert_eq!(resolution.state, State::AwaitingApp);
    assert_eq!(
        resolution.awaiting,
        Some(vec![Awaiting {
            transaction: 0,
            signers: vec![app.pubkey()]
        }])
    );
    let public = resolution.transactions[0].wire.clone().unwrap();
    assert!(decoded(&public).signers.iter().all(|slot| !slot.filled));
    assert_eq!(
        parse_wire(&public).unwrap().message,
        parse_wire(&signed).unwrap().message
    );
    // The wallet's signature stays in the wallet.
    assert_eq!(resolution.held, vec![signed.clone()]);
    let json = resolution.public();
    assert!(json.get("held").is_none());
    assert_eq!(json["awaiting"][0]["signers"][0], app.pubkey().to_string());

    // It can still be cancelled, and the wallet's signature goes.
    let mut cancelled = resolution.clone();
    cancelled.cancel(NOW).unwrap();
    assert!(cancelled.held.is_empty());

    // Saved and read back, it is the same.
    let saved = serde_json::to_string(&resolution).unwrap();
    assert_eq!(
        serde_json::from_str::<Resolution>(&saved).unwrap(),
        resolution
    );
}

#[test]
fn a_signature_given_has_to_be_owed_and_verify() {
    let wallet = KeypairSigner::new_random();
    let app = KeypairSigner::new_random();
    let signed = cosigned(&wallet, &app);
    let message = parse_wire(&signed).unwrap().message;
    let mut resolution = fresh(Delivery::Cosign, 1);
    resolution.signed(composed(signed), NOW).unwrap();

    let good: [u8; 64] = app.try_sign_message(&message).unwrap().into();
    let by_wallet: [u8; 64] = wallet.try_sign_message(&message).unwrap().into();
    let over_other: [u8; 64] = app.try_sign_message(b"something else").unwrap().into();

    let err = resolution
        .provide_signature(0, wallet.pubkey(), &by_wallet, NOW)
        .unwrap_err();
    assert_eq!(err.code(), "bad_signature");
    let err = resolution
        .provide_signature(1, app.pubkey(), &good, NOW)
        .unwrap_err();
    assert_eq!(err.code(), "bad_signature");
    let err = resolution
        .provide_signature(0, app.pubkey(), &over_other, NOW)
        .unwrap_err();
    assert_eq!(err.code(), "bad_signature");
    assert_eq!(resolution.state, State::AwaitingApp);

    resolution
        .provide_signature(0, app.pubkey(), &good, NOW + 5)
        .unwrap();
    assert_eq!(resolution.state, State::Sending);
    assert_eq!(resolution.awaiting, None);
    assert_eq!(resolution.transactions[0].wire, None);
    let wires = resolution.wires_to_send().unwrap();
    assert!(decoded(&wires[0]).fully_signed());
    assert_eq!(
        resolution.transactions[0].signature,
        parse_wire(&wires[0]).unwrap().signature()
    );
    // Nothing more is owed.
    assert_eq!(
        resolution
            .provide_signature(0, app.pubkey(), &good, NOW)
            .unwrap_err()
            .code(),
        "bad_state"
    );
}

#[test]
fn returned_the_app_gets_the_wallets_signatures() {
    let wallet = KeypairSigner::new_random();
    let app = KeypairSigner::new_random();
    let signed = cosigned(&wallet, &app);
    let mut resolution = fresh(Delivery::Return, 1);
    resolution.signed(composed(signed.clone()), NOW).unwrap();
    assert_eq!(resolution.state, State::Returned);
    assert_eq!(resolution.transactions[0].wire, Some(signed));
    assert_eq!(
        resolution.awaiting,
        Some(vec![Awaiting {
            transaction: 0,
            signers: vec![app.pubkey()]
        }])
    );
    assert!(resolution.held.is_empty());
    assert!(resolution.cancel(NOW).is_err());

    // Watched, what the app sent landed.
    let landed = Signature::from([4; 64]);
    resolution
        .sent(vec![SendResult::Landed { signature: landed }], NOW)
        .unwrap();
    assert_eq!(resolution.state, State::Done);
    assert_eq!(resolution.awaiting, None);
}

fn sending(roles: &[(TxRole, Vec<usize>)], requested: usize) -> Resolution {
    let wallet = KeypairSigner::new_random();
    let wire = apply_local_signatures(
        &compile_v1(
            wallet.pubkey(),
            vec![transfer(wallet.pubkey(), pk(9), 5)],
            None,
            Some(blockhash(1)),
        )
        .unwrap()
        .wire,
        &[&wallet as &dyn WalletSigner],
    )
    .unwrap();
    let mut resolution = fresh(Delivery::Send, requested);
    let made = roles
        .iter()
        .map(|(role, of)| Composed {
            role: *role,
            of: of.clone(),
            wire: wire.clone(),
        })
        .collect();
    resolution.signed(made, NOW).unwrap();
    resolution
}

#[test]
fn sent_and_landed_it_is_done_and_ran_says_where() {
    let mut resolution = sending(&[(TxRole::Direct, vec![0]), (TxRole::Carrier, vec![1])], 2);
    let (a, b) = (Signature::from([1; 64]), Signature::from([2; 64]));
    resolution
        .sent(
            vec![
                SendResult::Landed { signature: a },
                SendResult::Landed { signature: b },
            ],
            NOW + 9,
        )
        .unwrap();
    assert_eq!(resolution.state, State::Done);
    assert_eq!(
        resolution.ran,
        vec![
            Ran {
                transaction: 0,
                signature: a
            },
            Ran {
                transaction: 1,
                signature: b
            }
        ]
    );
    assert!(resolution.held.is_empty());
    assert_eq!(resolution.public()["ran"][1]["signature"], b.to_string());
    assert_eq!(resolution.public()["transactions"][0]["state"], "landed");
}

#[test]
fn what_may_still_land_is_unknown_and_never_failed() {
    let mut resolution = sending(&[(TxRole::Direct, vec![0]), (TxRole::Direct, vec![1])], 2);
    resolution
        .sent(
            vec![
                SendResult::Failed {
                    signature: None,
                    reason: "custom program error: 0x1".into(),
                },
                SendResult::Unknown { signature: None },
            ],
            NOW,
        )
        .unwrap();
    assert_eq!(resolution.state, State::Unknown);

    let mut failed = sending(&[(TxRole::Direct, vec![0])], 1);
    failed
        .sent(
            vec![SendResult::Failed {
                signature: Some(Signature::from([3; 64])),
                reason: "custom program error: 0x1".into(),
            }],
            NOW,
        )
        .unwrap();
    assert_eq!(failed.state, State::Failed);
    assert_eq!(failed.transactions[0].state, TxState::Failed);
    assert_eq!(
        failed.error.as_ref().map(|error| error.message.as_str()),
        Some("custom program error: 0x1")
    );

    // One result for each transaction.
    let mut short = sending(&[(TxRole::Direct, vec![0])], 1);
    assert!(short.sent(Vec::new(), NOW).is_err());
}

#[test]
fn a_proposal_that_landed_waits_until_it_runs() {
    let mut resolution = sending(&[(TxRole::Proposal, vec![0])], 1);
    let created = Signature::from([5; 64]);
    resolution
        .sent(vec![SendResult::Landed { signature: created }], NOW)
        .unwrap();
    assert_eq!(resolution.state, State::AwaitingSignatures);
    assert!(resolution.ran.is_empty());

    let ran = Signature::from([6; 64]);
    resolution.executed(0, ran, NOW + 60).unwrap();
    assert_eq!(resolution.state, State::Done);
    assert_eq!(
        resolution.ran,
        vec![Ran {
            transaction: 0,
            signature: ran
        }]
    );
    assert!(resolution.executed(0, ran, NOW).is_err());

    let mut closed = sending(&[(TxRole::Proposal, vec![0])], 1);
    closed
        .sent(vec![SendResult::Landed { signature: created }], NOW)
        .unwrap();
    closed
        .proposal_closed("The proposal was rejected.", NOW)
        .unwrap();
    assert_eq!(closed.state, State::Failed);
}

#[test]
fn approved_here_and_signed_elsewhere_it_goes_out() {
    let wallet = KeypairSigner::new_random();
    let mut resolution = fresh(Delivery::Send, 1);
    resolution.timing = Timing::Async;
    resolution.approved_elsewhere(NOW).unwrap();
    assert_eq!(resolution.state, State::AwaitingSignatures);
    // Approved, it no longer expires.
    assert!(resolution.expire(NOW).is_err());

    let unsigned = compile_v1(
        wallet.pubkey(),
        vec![transfer(wallet.pubkey(), pk(9), 5)],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    // Sent with a slot still empty, it is refused and waits as it was.
    assert!(resolution
        .signed_elsewhere(composed(unsigned.clone()), NOW)
        .is_err());
    assert_eq!(resolution.state, State::AwaitingSignatures);
    let signed = apply_local_signatures(&unsigned, &[&wallet as &dyn WalletSigner]).unwrap();
    resolution.signed_elsewhere(composed(signed), NOW).unwrap();
    assert_eq!(resolution.state, State::Sending);
}

#[test]
fn a_memo_resolution_says_so() {
    let resolution =
        Resolution::for_memo("m-1", pk(2), "solana:devnet", Timing::Async, "hello", NOW);
    assert_eq!(resolution.delivery, Delivery::Send);
    assert_eq!(resolution.public()["message"]["memo"], "hello");
}

#[test]
fn the_approval_window_follows_the_timing() {
    assert_eq!(Timing::Sync.approval_window_secs(), 600);
    assert_eq!(Timing::Async.approval_window_secs(), 86_400);
}

#[test]
fn one_an_earlier_failure_kept_back_has_no_signature_to_look_for() {
    let wallet = KeypairSigner::new_random();
    let signed = |to: u8| {
        let wire = compile_v1(
            wallet.pubkey(),
            vec![transfer(wallet.pubkey(), pk(to), 5)],
            None,
            Some(blockhash(1)),
        )
        .unwrap()
        .wire;
        apply_local_signatures(&wire, &[&wallet as &dyn WalletSigner]).unwrap()
    };
    let mut resolution = fresh(Delivery::Send, 2);
    let made = vec![
        Composed {
            role: TxRole::Direct,
            of: vec![0],
            wire: signed(8),
        },
        Composed {
            role: TxRole::Direct,
            of: vec![1],
            wire: signed(9),
        },
    ];
    resolution.signed(made, NOW).unwrap();
    assert!(resolution.transactions[1].signature.is_some());
    resolution
        .sent(
            vec![
                SendResult::Failed {
                    signature: None,
                    reason: "it failed".into(),
                },
                SendResult::NotSent {
                    reason: "Not sent.".into(),
                },
            ],
            NOW + 1,
        )
        .unwrap();
    assert_eq!(resolution.state, State::Failed);
    // The one that went out keeps its signature, the one kept back has none.
    assert!(resolution.transactions[0].signature.is_some());
    assert_eq!(resolution.transactions[1].state, TxState::Failed);
    assert_eq!(resolution.transactions[1].signature, None);
    assert_eq!(resolution.error.unwrap().message, "it failed");
}
