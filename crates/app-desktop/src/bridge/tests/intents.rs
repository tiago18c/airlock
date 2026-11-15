//! The intents prototype's bridge messages (`docs/intents.md`): each method,
//! each refusal in its order, the push, and what the window's answer does to
//! a resolution.

use super::*;
use app_core::resolutions::MAX_RESOLUTIONS;
use bridge_host::frame::{read_frame, write_frame};
use wallet_core::intent::State;

/// A request of the prototype from the test origin.
fn intent(id: &str, method: &str, params: Value) -> Value {
    json!({ "v": 1, "id": id, "origin": ORIGIN, "method": method, "params": params })
}

/// A `resolve` acting as `account` with `wires`, on devnet.
fn resolve_params(account: Pubkey, wires: &[Vec<u8>]) -> Value {
    let transactions: Vec<String> = wires.iter().map(|wire| v1_b64(wire)).collect();
    json!({
        "account": account.to_string(),
        "chain": "solana:devnet",
        "transactions": transactions,
    })
}

/// A hot wallet of this device, connected to the test origin.
fn hot_wallet(kit: &Kit, bridge: &Bridge) -> Pubkey {
    let wallet = with_hot_wallets(kit, &[7])[0];
    connected_as(kit, bridge, wallet, false);
    wallet
}

/// A legacy transaction paid by `payer` that creates `new`, which signs it.
fn creates(payer: &Pubkey, new: &Pubkey) -> Vec<u8> {
    let mut message = solana_message::Message::new(
        &[solana_system_interface::instruction::create_account(
            payer,
            new,
            1_000_000,
            0,
            &wallet_core::SYSTEM_PROGRAM_ID,
        )],
        Some(payer),
    );
    message.recent_blockhash = solana_hash::Hash::new_from_array([1; 32]);
    let signers = usize::from(message.header.num_required_signatures);
    wincode::serialize(&solana_transaction::versioned::VersionedTransaction {
        signatures: vec![solana_signature::Signature::default(); signers],
        message: solana_message::VersionedMessage::Legacy(message),
    })
    .unwrap()
}

fn code(reply: &Value) -> &str {
    reply["error"]["code"].as_str().unwrap_or("none")
}

/// A key the site holds, for a new account or a signer of its own.
fn site_key() -> wallet_core::KeypairSigner {
    wallet_core::KeypairSigner::from_seed(&[33; 32]).unwrap()
}

#[tokio::test]
async fn a_resolve_is_answered_at_once_and_waits_in_the_queue() {
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    let mint = site_key().pubkey();
    let mut params = resolve_params(wallet, &[creates(&wallet, &mint)]);
    params["id"] = json!("mint-1");
    params["accounts"] = json!({ mint.to_string(): { "kind": "new", "replaceable": true } });

    let peer = Peer::new(bridge.clone(), None);
    let answer = sent(&peer, intent("r1", "resolve", params));
    assert!(matches!(answer, Answer::Now(_)), "nobody is asked first");
    let reply = read(&settled(answer));
    let resolution = &reply["result"]["resolution"];
    assert_eq!(resolution["id"], "mint-1");
    assert_eq!(resolution["state"], "awaiting-approval");
    assert_eq!(resolution["account"], wallet.to_string());
    assert_eq!(resolution["timing"], "sync");
    assert_eq!(resolution["delivery"], "send");
    assert!(resolution.get("held").is_none());

    let queue = kit.core.slices().bridge.get();
    let head = queue.head().unwrap();
    assert_eq!(head.id, "r1");
    assert_eq!(head.method(), "resolve");
    assert!(head.sends());
    let call = head.resolve().unwrap();
    assert_eq!(call.plan.new_keys, vec![mint]);
    assert!(call.notes_checked);
    assert_eq!(call.transactions[0].chain.as_deref(), Some("solana:devnet"));
    // What the window shows of it is what it shows of a send.
    let prompt = BridgePrompt::head_of(&queue).unwrap();
    assert!(prompt.sends);
    assert_eq!(prompt.transactions, 1);

    // A page closing does not end it: the site can come back for it.
    peer.closed();
    assert_eq!(kit.core.slices().bridge.get().len(), 1);
}

#[tokio::test]
async fn a_resolve_with_no_id_gets_one() {
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    let reply = ask(
        &bridge,
        intent(
            "7f3ac0de-11",
            "resolve",
            resolve_params(wallet, &[paid_by(&wallet, &wallet)]),
        ),
    );
    let id = reply["result"]["resolution"]["id"].as_str().unwrap();
    assert!(wallet_core::intent::valid_id(id));
    let read_back = ask(&bridge, intent("g", "getResolution", json!({ "id": id })));
    assert_eq!(read_back["result"]["resolution"]["id"], id);
}

#[tokio::test]
async fn a_resolve_is_refused_in_the_order_the_reference_gives() {
    let (kit, bridge) = attached_bridge();
    let stranger = ask(
        &bridge,
        intent(
            "s",
            "resolve",
            resolve_params(account(), &[paid_by(&account(), &account())]),
        ),
    );
    assert_eq!(code(&stranger), "rejected");

    let wallets = with_hot_wallets(&kit, &[7, 8]);
    let (wallet, other) = (wallets[0], wallets[1]);
    connected_as(&kit, &bridge, wallet, false);
    let refused = |params: Value| ask(&bridge, intent("x", "resolve", params));

    // Another wallet of this device is asked, whatever the account named.
    let reply = refused(resolve_params(wallet, &[paid_by(&other, &wallet)]));
    assert_eq!(code(&reply), "rejected");
    assert_eq!(reply["error"]["message"], NOT_GIVEN_TO_SITE);
    // Unreadable, and with something wrong after it too: unreadable first.
    let mut unreadable = resolve_params(wallet, &[paid_by(&wallet, &wallet)]);
    unreadable["order"] = json!("whenever");
    unreadable["accounts"] = json!({ "nope": { "kind": "new", "replaceable": true } });
    assert_eq!(code(&refused(unreadable)), "bad_request");
    let mut no_chain = resolve_params(wallet, &[paid_by(&wallet, &wallet)]);
    no_chain.as_object_mut().unwrap().remove("chain");
    assert_eq!(code(&refused(no_chain)), "bad_request");
    let mut bad_id = resolve_params(wallet, &[paid_by(&wallet, &wallet)]);
    bad_id["id"] = json!("not ok");
    assert_eq!(code(&refused(bad_id)), "bad_request");
    assert_eq!(code(&refused(resolve_params(wallet, &[]))), "bad_request");
    let many = vec![paid_by(&wallet, &wallet); MAX_TRANSACTIONS + 1];
    assert_eq!(
        code(&refused(resolve_params(wallet, &many))),
        "not_implemented"
    );
    let garbled = resolve_params(wallet, &[vec![1, 2, 3]]);
    assert_eq!(code(&refused(garbled)), "bad_request");

    // A note that cannot be read, or that fails a check, before what the
    // account cannot do.
    let own = paid_by(&wallet, &wallet);
    let with_notes = |notes: Value, delivery: &str| {
        let mut params = resolve_params(wallet, std::slice::from_ref(&own));
        params["accounts"] = notes;
        params["delivery"] = json!(delivery);
        params["order"] = json!("atomic");
        refused(params)
    };
    let reply = with_notes(
        json!({ "nope": { "kind": "new", "replaceable": true } }),
        "send",
    );
    assert_eq!(code(&reply), "bad_notes");
    let reply = with_notes(
        json!({ Pubkey::new_from_array([60; 32]).to_string(): { "kind": "signer", "signs": "app" } }),
        "send",
    );
    assert_eq!(code(&reply), "bad_notes");
    // The account acted as carries no note, even as its own fee payer.
    let reply = with_notes(
        json!({ wallet.to_string(): { "kind": "feePayer", "replaceable": true } }),
        "send",
    );
    assert_eq!(code(&reply), "bad_notes");
    assert!(reply["error"]["message"]
        .as_str()
        .unwrap()
        .contains("carries no note"));

    // What the account cannot do names the requirement.
    let app = site_key().pubkey();
    let signs = paid_by(&wallet, &app);
    let mut cosigner = resolve_params(wallet, std::slice::from_ref(&signs));
    cosigner["accounts"] = json!({ app.to_string(): { "kind": "signer", "signs": "app" } });
    let reply = refused(cosigner.clone());
    assert_eq!(code(&reply), "unsupported");
    assert_eq!(reply["error"]["requirement"], "signer");
    let mut two = resolve_params(wallet, &[own.clone(), own.clone()]);
    two["order"] = json!("atomic");
    let reply = refused(two);
    assert_eq!(reply["error"]["requirement"], "order");
    let mut elsewhere = resolve_params(wallet, std::slice::from_ref(&own));
    elsewhere["chain"] = json!("solana:localnet");
    let reply = refused(elsewhere);
    assert_eq!(reply["error"]["requirement"], "chain");

    // And last, an id the site used before.
    let mut first = resolve_params(wallet, std::slice::from_ref(&own));
    first["id"] = json!("once");
    assert_eq!(code(&refused(first.clone())), "none");
    assert_eq!(code(&refused(first)), "duplicate_id");
    assert_eq!(kit.core.slices().bridge.get().len(), 1);
}

#[tokio::test]
async fn a_v0_transaction_that_reads_tables_has_its_notes_checked_later() {
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    // The v0 transfer with a lookup of a table nobody has read.
    let mut wire = v0_transfer_wire(&wallet);
    let tx: solana_transaction::versioned::VersionedTransaction =
        wincode::deserialize(&wire).unwrap();
    let solana_message::VersionedMessage::V0(mut message) = tx.message else {
        panic!("a v0 message");
    };
    message
        .address_table_lookups
        .push(solana_message::v0::MessageAddressTableLookup {
            account_key: Pubkey::new_from_array([50; 32]),
            writable_indexes: vec![],
            readonly_indexes: vec![0],
        });
    wire = wincode::serialize(&solana_transaction::versioned::VersionedTransaction {
        signatures: tx.signatures,
        message: solana_message::VersionedMessage::V0(message),
    })
    .unwrap();
    let reply = ask(
        &bridge,
        intent("v0", "resolve", resolve_params(wallet, &[wire.clone()])),
    );
    assert_eq!(
        reply["result"]["resolution"]["state"], "awaiting-approval",
        "{reply}"
    );
    let queue = kit.core.slices().bridge.get();
    assert!(!queue.head().unwrap().resolve().unwrap().notes_checked);

    // A note on the account acted as needs no table to be refused.
    let mut on_account = resolve_params(wallet, &[wire]);
    on_account["accounts"] =
        json!({ wallet.to_string(): { "kind": "feePayer", "replaceable": true } });
    assert_eq!(
        code(&ask(&bridge, intent("v0b", "resolve", on_account))),
        "bad_notes"
    );
}

#[tokio::test]
async fn a_profile_goes_to_a_connected_site_only() {
    let (kit, bridge) = attached_bridge();
    let wallet = with_hot_wallets(&kit, &[7])[0];
    let ask_profile = |id: &str| {
        ask(
            &bridge,
            intent(
                id,
                "getSigningProfile",
                json!({ "account": wallet.to_string() }),
            ),
        )
    };
    assert_eq!(code(&ask_profile("p0")), "rejected");
    connected_as(&kit, &bridge, wallet, false);
    let reply = ask_profile("p1");
    let profile = &reply["result"]["profile"];
    assert_eq!(profile["account"], wallet.to_string());
    assert_eq!(profile["timing"], "sync");
    assert_eq!(profile["execution"], "top-level");
    assert_eq!(profile["feePayer"], json!(["account", "wallet", "app"]));
    assert_eq!(profile["newAccounts"], json!(["wallet-key", "app-key"]));
    assert_eq!(profile["delivery"], json!(["send", "cosign", "return"]));
    assert_eq!(profile["messages"], json!(["signature", "memo"]));
    assert_eq!(profile["maxTransactions"], 16);
    kit.core.site_block(ORIGIN);
    assert_eq!(code(&ask_profile("p2")), "rejected");
}

#[tokio::test]
async fn a_site_reads_only_its_own_resolutions() {
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    let mut params = resolve_params(wallet, &[paid_by(&wallet, &wallet)]);
    params["id"] = json!("mine");
    ask(&bridge, intent("r", "resolve", params));

    let mine = ask(
        &bridge,
        intent("g1", "getResolution", json!({ "id": "mine" })),
    );
    assert_eq!(mine["result"]["resolution"]["state"], "awaiting-approval");
    assert_eq!(
        code(&ask(
            &bridge,
            intent("g2", "getResolution", json!({ "id": "other" }))
        )),
        "not_found"
    );
    let mut theirs = intent("g3", "getResolution", json!({ "id": "mine" }));
    theirs["origin"] = json!("https://other.test");
    assert_eq!(code(&ask(&bridge, theirs)), "not_found");
    let mut cancel = intent("c", "cancelResolution", json!({ "id": "mine" }));
    cancel["origin"] = json!("https://other.test");
    assert_eq!(code(&ask(&bridge, cancel)), "not_found");
    assert_eq!(kit.core.slices().bridge.get().len(), 1);
    assert_eq!(
        code(&ask(&bridge, intent("g4", "getResolution", json!({})))),
        "bad_request"
    );
}

#[tokio::test]
async fn cancelling_an_undecided_resolve_takes_it_out_of_the_queue() {
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    let mut params = resolve_params(wallet, &[paid_by(&wallet, &wallet)]);
    params["id"] = json!("c1");
    ask(&bridge, intent("r", "resolve", params));
    let reply = ask(
        &bridge,
        intent("c", "cancelResolution", json!({ "id": "c1" })),
    );
    assert_eq!(reply["result"]["resolution"]["state"], "cancelled");
    assert!(kit.core.slices().bridge.get().is_empty());
    let again = ask(
        &bridge,
        intent("c2", "cancelResolution", json!({ "id": "c1" })),
    );
    assert_eq!(code(&again), "bad_state");
}

#[tokio::test]
async fn cosigned_the_app_gives_its_signatures_from_the_page() {
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    let app = site_key();
    let wire = paid_by(&wallet, &app.pubkey());
    let mut params = resolve_params(wallet, std::slice::from_ref(&wire));
    params["id"] = json!("co");
    params["delivery"] = json!("cosign");
    params["accounts"] = json!({ app.pubkey().to_string(): { "kind": "signer", "signs": "app" } });
    let reply = ask(&bridge, intent("r", "resolve", params));
    assert_eq!(
        reply["result"]["resolution"]["delivery"], "cosign",
        "{reply}"
    );
    let queue = kit.core.slices().bridge.get();
    let head = queue.head().unwrap();
    assert!(!head.sends());
    assert_eq!(head.resolve().unwrap().plan.app_signs, vec![app.pubkey()]);

    let provide = |id: &str, signatures: Value| {
        ask(
            &bridge,
            intent(
                id,
                "provideSignatures",
                json!({ "id": "co", "signatures": signatures }),
            ),
        )
    };
    let early = provide(
        "p0",
        json!([{ "transaction": 0, "signer": app.pubkey().to_string(), "signature": solana_signature::Signature::from([1; 64]).to_string() }]),
    );
    assert_eq!(code(&early), "bad_state");
    assert_eq!(code(&provide("p1", json!([]))), "bad_request");
    assert_eq!(
        code(&provide(
            "p2",
            json!([{ "transaction": 0, "signer": "x", "signature": "y" }])
        )),
        "bad_request"
    );

    // The window signs: the wallet's key, the app's slot left empty.
    let key = wallet_core::KeypairSigner::from_seed(&[7; 32]).unwrap();
    let signed = wallet_core::apply_local_signatures(&wire, &[&key as &dyn WalletSigner]).unwrap();
    assert!(kit
        .core
        .bridge_answer("r", &BridgeAnswer::Signed(vec![signed])));
    let waiting = ask(&bridge, intent("g", "getResolution", json!({ "id": "co" })));
    let resolution = &waiting["result"]["resolution"];
    assert_eq!(resolution["state"], "awaiting-app");
    assert_eq!(
        resolution["awaiting"][0]["signers"][0],
        app.pubkey().to_string()
    );
    // What the app is handed has every slot empty.
    let handed = base64::engine::general_purpose::STANDARD
        .decode(resolution["transactions"][0]["wire"].as_str().unwrap())
        .unwrap();
    let view = wallet_core::parse_wire(&handed).unwrap();
    assert!((0..view.num_required_signatures).all(|slot| !view.slot_valid(slot)));

    // A signature by someone not owed is refused.
    let stranger = wallet_core::KeypairSigner::from_seed(&[44; 32]).unwrap();
    let message = view.message.clone();
    let theirs = stranger.try_sign_message(&message).unwrap();
    let reply = provide(
        "p3",
        json!([{ "transaction": 0, "signer": stranger.pubkey().to_string(), "signature": theirs.to_string() }]),
    );
    assert_eq!(code(&reply), "bad_signature");

    let signature = app.try_sign_message(&message).unwrap();
    let reply = provide(
        "p4",
        json!([{ "transaction": 0, "signer": app.pubkey().to_string(), "signature": signature.to_string() }]),
    );
    assert_eq!(reply["result"]["resolution"]["state"], "sending", "{reply}");
    assert_eq!(
        kit.core.slices().resolutions.get().ready_to_send().count(),
        1
    );
}

#[tokio::test]
async fn a_message_is_a_memo_or_waits_for_a_key() {
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    let message = |id: &str, text: &[u8], mode: Option<&str>| {
        let mut params = json!({
            "account": wallet.to_string(),
            "message": base64::engine::general_purpose::STANDARD.encode(text),
        });
        if let Some(mode) = mode {
            params["mode"] = json!(mode);
        }
        intent(id, "resolveMessage", params)
    };

    let reply = ask(
        &bridge,
        message("m1", b"example.com wants you to sign: 42", Some("memo")),
    );
    assert_eq!(reply["result"]["kind"], "memo", "{reply}");
    let resolution = &reply["result"]["resolution"];
    assert_eq!(resolution["state"], "awaiting-approval");
    assert_eq!(
        resolution["message"]["memo"],
        "example.com wants you to sign: 42"
    );
    let queue = kit.core.slices().bridge.get();
    let head = queue.head().unwrap();
    assert_eq!(head.method(), "resolveMessage");
    assert!(head.sends());
    let call = head.resolve().unwrap();
    let tx = wallet_core::decode_transaction(
        call.transactions[0].wire.bytes().unwrap(),
        &wallet_core::LookupTables::new(),
    )
    .unwrap();
    assert!(wallet_core::intent::is_memo_of(
        &tx,
        &wallet,
        b"example.com wants you to sign: 42"
    ));

    let binary = ask(&bridge, message("m2", &[0xff, 0xfe], Some("memo")));
    assert_eq!(code(&binary), "unsupported");
    assert_eq!(binary["error"]["requirement"], "messages");
    assert_eq!(
        code(&ask(&bridge, message("m3", b"hi", Some("ink")))),
        "bad_request"
    );

    // With a key, and no mode named, it waits for the person as signMessage
    // does, and answers with the signature in base58.
    let peer = Peer::new(bridge.clone(), None);
    let waiting = sent(&peer, message("m4", b"hello", None));
    assert!(matches!(waiting, Answer::Later(_)));
    assert_eq!(kit.core.slices().bridge.get().len(), 2);
    assert!(kit.core.bridge_answer(
        "m4",
        &BridgeAnswer::SignedMessage {
            signed: b"hello".to_vec(),
            signature: [5; 64],
        }
    ));
    let reply = read(&settled(waiting));
    assert_eq!(reply["result"]["kind"], "signature");
    assert_eq!(
        reply["result"]["signature"],
        solana_signature::Signature::from([5; 64]).to_string()
    );
    assert_eq!(
        reply["result"]["signedMessage"],
        base64::engine::general_purpose::STANDARD.encode(b"hello")
    );
    let refused = sent(&peer, message("m5", b"hello", Some("signature")));
    assert!(matches!(refused, Answer::Later(_)));
    assert!(kit.core.bridge_answer("m5", &BridgeAnswer::Rejected));
    assert_eq!(code(&read(&settled(refused))), "rejected");
}

/// A memo is a transaction, so it is for the chain the site names, as any
/// request is. With none named it is for the network Airlock is on, and a
/// node Airlock cannot name (a local validator) leaves it nothing to say.
#[tokio::test]
async fn a_memo_is_for_the_chain_the_site_names() {
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    kit.core.workspace_deferred("switch", |ws| {
        ws.use_cluster(wallet_core::Cluster::custom("local", "http://127.0.0.1:8899").unwrap());
    });
    let memo = |id: &str, chain: Option<Value>| {
        let mut params = json!({
            "account": wallet.to_string(),
            "message": base64::engine::general_purpose::STANDARD.encode(b"hi"),
            "mode": "memo",
        });
        if let Some(chain) = chain {
            params["chain"] = chain;
        }
        ask(&bridge, intent(id, "resolveMessage", params))
    };

    let named = memo("c1", Some(json!("solana:devnet")));
    assert_eq!(
        named["result"]["resolution"]["chain"], "solana:devnet",
        "{named}"
    );
    let head = kit.core.slices().bridge.get();
    let call = head.head().unwrap().resolve().unwrap().clone();
    assert_eq!(call.chain, "solana:devnet");
    assert_eq!(call.transactions[0].chain.as_deref(), Some("solana:devnet"));

    let unnamed = memo("c2", None);
    assert_eq!(code(&unnamed), "unsupported", "{unnamed}");
    assert_eq!(unnamed["error"]["requirement"], "chain");
    let other = memo("c3", Some(json!("solana:localnet")));
    assert_eq!(code(&other), "unsupported");
    assert_eq!(other["error"]["requirement"], "chain");
    assert_eq!(code(&memo("c4", Some(json!(7)))), "bad_request");

    // On devnet, a memo that names no chain is for devnet.
    kit.core.workspace_deferred("switch", |ws| {
        ws.use_cluster(wallet_core::Cluster::Devnet);
    });
    let on_devnet = memo("c5", None);
    assert_eq!(
        on_devnet["result"]["resolution"]["chain"], "solana:devnet",
        "{on_devnet}"
    );
}

#[tokio::test]
async fn the_windows_answer_moves_the_resolution_on() {
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    let wire = paid_by(&wallet, &wallet);
    let start = |id: &str| {
        let mut params = resolve_params(wallet, std::slice::from_ref(&wire));
        params["id"] = json!(id);
        ask(&bridge, intent(id, "resolve", params));
    };
    let state = |id: &str| {
        ask(&bridge, intent("g", "getResolution", json!({ "id": id })))["result"]["resolution"]
            ["state"]
            .clone()
    };

    start("no");
    assert!(kit.core.bridge_answer("no", &BridgeAnswer::Rejected));
    assert_eq!(state("no"), "declined");

    start("yes");
    let key = wallet_core::KeypairSigner::from_seed(&[7; 32]).unwrap();
    let signed = wallet_core::apply_local_signatures(&wire, &[&key as &dyn WalletSigner]).unwrap();
    // A wire still missing a signature does not go out, and it keeps waiting.
    assert!(kit
        .core
        .bridge_begin_send("yes", std::slice::from_ref(&wire))
        .is_none());
    assert_eq!(state("yes"), "awaiting-approval");
    let sending = kit
        .core
        .bridge_begin_send("yes", std::slice::from_ref(&signed))
        .unwrap();
    assert_eq!(state("yes"), "sending");
    kit.core.bridge_send_finished(sending, vec![Ok([3; 64])]);
    let done = ask(
        &bridge,
        intent("g", "getResolution", json!({ "id": "yes" })),
    );
    let resolution = &done["result"]["resolution"];
    assert_eq!(resolution["state"], "done");
    assert_eq!(resolution["transactions"][0]["role"], "direct");
    assert_eq!(
        resolution["ran"][0]["signature"],
        solana_signature::Signature::from([3; 64]).to_string()
    );

    start("maybe");
    let sending = kit
        .core
        .bridge_begin_send("maybe", std::slice::from_ref(&signed))
        .unwrap();
    kit.core
        .bridge_send_finished(sending, vec![Err(SendError::Unknown("not seen".into()))]);
    assert_eq!(state("maybe"), "unknown");

    // Disconnecting the site declines what it has waiting.
    start("left");
    kit.core.site_disconnect(ORIGIN);
    assert_eq!(state("left"), "declined");
    assert!(kit.core.slices().bridge.get().is_empty());
}

#[tokio::test]
async fn a_resolution_expires_after_its_window_with_nobody_deciding() {
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    let mut params = resolve_params(wallet, &[paid_by(&wallet, &wallet)]);
    params["id"] = json!("late");
    ask(&bridge, intent("r", "resolve", params));
    app_core::testing::settle().await;
    kit.clock.advance(Duration::from_secs(10 * 60));
    app_core::testing::settle().await;
    let reply = ask(
        &bridge,
        intent("g", "getResolution", json!({ "id": "late" })),
    );
    assert_eq!(reply["result"]["resolution"]["state"], "expired");
    assert!(kit.core.slices().bridge.get().is_empty());
}

#[test]
fn a_push_goes_for_each_resolution_that_changed() {
    let view = |state: &str| json!({ "id": "a", "state": state });
    let key = |origin: &str, id: &str| (origin.to_string(), id.to_string());
    let told = BTreeMap::from([
        (key("https://a.test", "a"), view("awaiting-approval")),
        (key("https://b.test", "a"), view("sending")),
    ]);
    let now = BTreeMap::from([
        (key("https://a.test", "a"), view("declined")),
        (key("https://b.test", "a"), view("sending")),
        (key("https://c.test", "a"), view("awaiting-approval")),
    ]);
    let frames: Vec<Value> = crate::bridge::intents::resolution_changes(&told, &now)
        .iter()
        .map(|frame| read(frame))
        .collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["method"], "resolutionChanged");
    assert_eq!(frames[0]["origin"], "https://a.test");
    assert_eq!(frames[0]["params"]["resolution"]["state"], "declined");
    assert_eq!(frames[1]["origin"], "https://c.test");
    assert!(frames[0].get("id").is_none());
    // One dropped to make room says nothing.
    assert!(crate::bridge::intents::resolution_changes(&now, &BTreeMap::new()).is_empty());
}

/// Frames the app sends a client of `endpoint`, after `first` is written.
fn client(endpoint: &Endpoint, first: &Value) -> mpsc::Receiver<Value> {
    let mut stream = bridge_host::endpoint::connect(&endpoint.name).unwrap();
    write_frame(&mut stream, &serde_json::to_vec(first).unwrap()).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(Some(frame)) = read_frame(&mut stream) {
            if tx.send(read(&frame)).is_err() {
                break;
            }
        }
    });
    rx
}

/// The next frame, letting the core's tasks run while it waits.
async fn next(frames: &mpsc::Receiver<Value>) -> Value {
    for _ in 0..500 {
        app_core::testing::settle().await;
        if let Ok(frame) = frames.try_recv() {
            return frame;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("no frame arrived");
}

#[tokio::test]
async fn every_browser_hears_when_a_resolution_changes() {
    let kit = kit();
    let endpoint = endpoint("intent-push");
    let Bootstrap::Primary(bridge) = Bridge::new().claim(&endpoint, SHOW_TIMEOUT) else {
        panic!("the socket was free");
    };
    bridge.attach(kit.core.public());
    let wallet = hot_wallet(&kit, &bridge);
    let mut params = resolve_params(wallet, &[paid_by(&wallet, &wallet)]);
    params["id"] = json!("pushed");

    let frames = client(&endpoint, &intent("r", "resolve", params));
    // The reply, and the push that says it was made, which is a change too.
    // They are written by two threads, so either may come first.
    let (first, second) = (next(&frames).await, next(&frames).await);
    let (reply, made) = if first["id"] == "r" {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(reply["id"], "r");
    assert_eq!(made["method"], "resolutionChanged");
    assert_eq!(made["params"]["resolution"]["state"], "awaiting-approval");

    assert!(kit.core.bridge_answer("r", &BridgeAnswer::Rejected));
    let push = next(&frames).await;
    assert_eq!(push["method"], "resolutionChanged");
    assert_eq!(push["origin"], ORIGIN);
    assert_eq!(push["params"]["resolution"]["id"], "pushed");
    assert_eq!(push["params"]["resolution"]["state"], "declined");
    assert!(push["params"]["resolution"].get("held").is_none());
}

#[tokio::test]
async fn an_intent_request_needs_the_core_and_a_web_origin() {
    let bridge = Bridge::new();
    let reply = ask(&bridge, intent("b", "getResolution", json!({ "id": "x" })));
    assert_eq!(code(&reply), "busy");

    let (_kit, bridge) = attached_bridge();
    let peer = Peer::new(bridge.clone(), None);
    let mut nowhere = intent("o", "getResolution", json!({ "id": "x" }));
    nowhere["origin"] = json!("");
    assert_eq!(code(&read(&settled(sent(&peer, nowhere)))), "bad_request");
}

#[tokio::test]
async fn the_kept_resolutions_are_bounded() {
    // The store's own limit, through the bridge: nothing is refused for it.
    let (kit, bridge) = attached_bridge();
    let wallet = hot_wallet(&kit, &bridge);
    let wire = paid_by(&wallet, &wallet);
    for n in 0..MAX_RESOLUTIONS + 2 {
        let mut params = resolve_params(wallet, std::slice::from_ref(&wire));
        params["id"] = json!(format!("n{n}"));
        let reply = ask(&bridge, intent(&format!("q{n}"), "resolve", params));
        assert!(reply.get("error").is_none(), "{reply}");
        assert!(kit
            .core
            .bridge_answer(&format!("q{n}"), &BridgeAnswer::Rejected));
    }
    assert_eq!(kit.core.slices().resolutions.get().len(), MAX_RESOLUTIONS);
    let oldest = ask(&bridge, intent("g", "getResolution", json!({ "id": "n0" })));
    assert_eq!(code(&oldest), "not_found");
    // Reading what is kept is a state the extension knows.
    let latest = ask(
        &bridge,
        intent(
            "g",
            "getResolution",
            json!({ "id": format!("n{}", MAX_RESOLUTIONS + 1) }),
        ),
    );
    let state = latest["result"]["resolution"]["state"].as_str().unwrap();
    assert_eq!(state, State::Declined.name());
}

/// A multisig of this device's hot key and a key elsewhere that needs both,
/// connected to the test origin. Answers its vault.
fn waiting_multisig(kit: &Kit, bridge: &Bridge) -> Pubkey {
    let hot = with_hot_wallets(kit, &[7])[0];
    let hot_id = kit.core.with_wallets(|data| {
        data.unwrap()
            .wallets
            .iter()
            .find(|wallet| wallet.address() == Some(hot))
            .unwrap()
            .id
    });
    let squad = wallet_core::squads::Squad::v4(&Pubkey::new_from_array([61; 32]));
    let mut wallet = wallet_core::WalletRecord::squads(
        "Treasury",
        squad,
        vec![hot, Pubkey::new_from_array([62; 32])],
        2,
    );
    wallet.kind.set_member_wallet(Some(hot_id));
    wallet.kind.set_fee_payer(Some(hot_id));
    let vault = wallet.address().unwrap();
    kit.core
        .enroll(
            "add_contract_wallet",
            |data| data.add_contract_wallet(wallet),
            |_, _| {},
        )
        .unwrap();
    connected_as(kit, bridge, vault, false);
    vault
}

#[tokio::test]
async fn a_multisig_that_waits_takes_a_new_account_its_program_signs_for() {
    let (kit, bridge) = attached_bridge();
    let vault = waiting_multisig(&kit, &bridge);
    let profile = ask(
        &bridge,
        intent(
            "p",
            "getSigningProfile",
            json!({ "account": vault.to_string() }),
        ),
    );
    let profile = &profile["result"]["profile"];
    assert_eq!(profile["timing"], "async");
    assert_eq!(profile["newAccounts"], json!(["carrier"]));

    let mint = site_key().pubkey();
    let new = json!({ mint.to_string(): { "kind": "new", "replaceable": true } });
    // In one transaction, it waits in the queue for the person.
    let mut params = resolve_params(vault, &[creates(&vault, &mint)]);
    params["accounts"] = new.clone();
    let reply = ask(&bridge, intent("m1", "resolve", params));
    assert_eq!(
        reply["result"]["resolution"]["state"], "awaiting-approval",
        "{reply}"
    );
    let queue = kit.core.slices().bridge.get();
    let call = queue.head().unwrap().resolve().unwrap();
    assert_eq!(call.plan.carried, vec![mint]);
    assert!(call.plan.new_keys.is_empty());

    // In two, the program could sign for it in one proposal only.
    let mut twice = resolve_params(vault, &[creates(&vault, &mint), creates(&vault, &mint)]);
    twice["accounts"] = new;
    let reply = ask(&bridge, intent("m2", "resolve", twice));
    assert_eq!(code(&reply), "unsupported", "{reply}");
    assert_eq!(reply["error"]["requirement"], "newAccounts");
}
