use super::*;
use app_core::bridge::BridgeAnswer;
use app_core::testing::{kit, Kit};
use app_core::{BridgeRequestOutcome, NoticeKind};
use bridge_protocol::reply::SendError;
use wallet_core::WalletSigner;

/// A bridge attached to a test core.
fn attached_bridge() -> (Kit, Bridge) {
    let kit = kit();
    let bridge = Bridge::new();
    bridge.attach(kit.core.public());
    (kit, bridge)
}

/// The reply, however long it takes.
fn settled(answer: Answer) -> Vec<u8> {
    match answer {
        Answer::Now(bytes) => bytes,
        Answer::Later(wait) => wait(),
    }
}

fn ask(bridge: &Bridge, request: Value) -> Value {
    let message = serde_json::to_vec(&request).unwrap();
    serde_json::from_slice(&settled(bridge.answer(&message, &Waiting::default()))).unwrap()
}

/// `request` from the connection `peer`, as it is taken in.
fn sent(peer: &Peer, request: Value) -> Answer {
    peer.answer(&serde_json::to_vec(&request).unwrap())
}

fn read(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

fn connect(id: &str) -> BridgeRequest {
    BridgeRequest {
        id: id.into(),
        origin: "https://example.test".into(),
        method: "connect".into(),
        params: Value::Null,
    }
}

#[test]
fn status_accepts_an_expiry_in_unix_seconds() {
    let value = ask(
        &Bridge::new(),
        json!({
            "v": 1,
            "id": "abc",
            "method": "status",
            "expiresAt": now_secs() + 60
        }),
    );
    assert_eq!(value["result"]["role"], "desktop");
    assert_eq!(value["result"]["ready"], true);
}

#[test]
fn only_connect_and_sign_wait_for_the_user() {
    assert!(waits_for_user("connect"));
    assert!(waits_for_user("signTransaction"));
    assert!(waits_for_user("signMessage"));
    assert!(waits_for_user("signIn"));
    assert!(waits_for_user(SEND));
    assert!(waits_for_user(SEND_ALL));
    assert!(!waits_for_user("status"));
    assert!(!waits_for_user("show"));
    assert!(!waits_for_user("disconnect"));
}

#[test]
fn a_second_launch_can_show_the_window_before_the_core_opens() {
    let bridge = Bridge::new();
    let show = bridge.subscribe_show();
    let value = ask(&bridge, json!({ "v": 1, "id": "show", "method": "show" }));
    assert!(value.get("error").is_none());
    assert!(show.has_changed().unwrap());

    // A request for the user waits for the core instead.
    let value = ask(
        &bridge,
        json!({ "v": 1, "id": "c", "method": "connect", "origin": "https://example.test" }),
    );
    assert_eq!(value["error"]["code"], "busy");
}

#[tokio::test]
async fn a_request_nobody_answers_leaves_the_queue() {
    let (kit, _bridge) = attached_bridge();
    let bytes = settled(wait_for_user(
        &kit.core.public(),
        connect("stale"),
        Duration::from_millis(10),
        &Waiting::default(),
    ));
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["error"]["code"], "expired");
    assert!(kit.core.slices().bridge.get().is_empty());
    let expired = kit.core.slices().notices.get().items().iter().any(|n| {
        matches!(
            n.kind,
            NoticeKind::BridgeRequest {
                outcome: BridgeRequestOutcome::Expired,
                ..
            }
        )
    });
    assert!(expired);
}

#[tokio::test]
async fn the_window_answers_from_the_core_queue() {
    let account = Pubkey::new_from_array([7; 32]);
    let params = json!({
        "transactions": [{ "account": account.to_string(), "transaction": "gQ==" }]
    });
    assert_eq!(requested_account(&params, 0), Some(account));
    assert_eq!(requested_account(&params, 1), None);
    assert_eq!(requested_account(&json!({ "transactions": [{}] }), 0), None);
    assert_eq!(
        requested_account(
            &json!({ "transactions": [{ "account": "not an address" }] }),
            0
        ),
        None
    );

    let (kit, _bridge) = attached_bridge();
    let waiting = |id: &'static str| {
        let core = kit.core.public();
        std::thread::spawn(move || {
            settled(wait_for_user(
                &core,
                connect(id),
                Duration::from_secs(5),
                &Waiting::default(),
            ))
        })
    };
    let first = waiting("count-1");
    while kit.core.slices().bridge.get().is_empty() {
        std::thread::yield_now();
    }
    let second = waiting("count-2");
    while kit.core.slices().bridge.get().len() < 2 {
        std::thread::yield_now();
    }

    let shown = BridgePrompt::head_of(&kit.core.slices().bridge.get()).unwrap();
    assert_eq!(shown.id, "count-1");
    assert_eq!(shown.waiting, 1);

    assert!(kit.core.bridge_answer(
        "count-1",
        &BridgeAnswer::Connect(vec![bridge_protocol::reply::SiteAccount {
            pubkey: [1; 32],
            label: String::new(),
            signs_messages: false,
        }])
    ));
    assert!(kit.core.bridge_answer("count-2", &BridgeAnswer::Rejected));
    let first: Value = serde_json::from_slice(&first.join().unwrap()).unwrap();
    let second: Value = serde_json::from_slice(&second.join().unwrap()).unwrap();
    assert_eq!(first["id"], "count-1");
    assert_eq!(first["result"]["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(second["error"]["code"], "rejected");
    assert!(BridgePrompt::head_of(&kit.core.slices().bridge.get()).is_none());
}

const ORIGIN: &str = "https://example.test";

fn account() -> Pubkey {
    Pubkey::new_from_array([7; 32])
}

fn shareable(pubkey: Pubkey) -> wallet_core::ShareableAccount {
    wallet_core::ShareableAccount {
        pubkey,
        label: "Main".into(),
        contract: false,
        summary: String::new(),
    }
}

/// Connect the test origin to `account()` the way the modal does.
fn connected(kit: &Kit, bridge: &Bridge, remember: bool) {
    let asking = {
        let bridge = bridge.clone();
        std::thread::spawn(move || {
            ask(
                &bridge,
                json!({ "v": 1, "id": "connect", "origin": ORIGIN, "method": "connect" }),
            )
        })
    };
    while kit.core.slices().bridge.get().is_empty() {
        std::thread::yield_now();
    }
    assert!(kit
        .core
        .bridge_connect("connect", vec![shareable(account())], remember));
    asking.join().unwrap();
}

fn silent_connect(bridge: &Bridge) -> Value {
    ask(
        bridge,
        json!({ "v": 1, "id": "s", "origin": ORIGIN, "method": "connect", "params": { "silent": true } }),
    )
}

fn sign_request(id: &str, count: usize) -> Value {
    let v1 = base64::engine::general_purpose::STANDARD.encode([0x81, 1, 2]);
    let transactions: Vec<Value> = (0..count)
        .map(|_| json!({ "account": account().to_string(), "transaction": v1 }))
        .collect();
    json!({
        "v": 1,
        "id": id,
        "origin": "https://example.test",
        "method": "signTransaction",
        "params": { "transactions": transactions }
    })
}

#[tokio::test]
async fn an_empty_or_oversized_batch_is_refused() {
    let (kit, bridge) = attached_bridge();
    let value = ask(&bridge, sign_request("none", 0));
    assert_eq!(value["error"]["code"], "bad_request");
    let value = ask(&bridge, sign_request("many", MAX_TRANSACTIONS + 1));
    assert_eq!(value["error"]["code"], "not_implemented");
    assert!(kit.core.slices().bridge.get().is_empty());
}

#[tokio::test]
async fn a_batch_waits_as_one_request() {
    let (kit, bridge) = attached_bridge();
    connected(&kit, &bridge, false);
    let waiting = std::thread::spawn(move || ask(&bridge, sign_request("batch", 3)));
    while kit.core.slices().bridge.get().is_empty() {
        std::thread::yield_now();
    }
    let shown = BridgePrompt::head_of(&kit.core.slices().bridge.get()).unwrap();
    assert_eq!(shown.transactions, 3);
    assert_eq!(shown.waiting, 0);

    let wires = vec![vec![0x81, 1], vec![0x81, 2], vec![0x81, 3]];
    assert!(kit
        .core
        .bridge_answer("batch", &BridgeAnswer::Signed(wires)));
    let value = waiting.join().unwrap();
    assert_eq!(
        value["result"]["signedTransactions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

fn v1_b64(wire: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(wire)
}

fn params_for_wire(wire: &[u8]) -> Value {
    json!({
        "transactions": [{
            "transaction": base64::engine::general_purpose::STANDARD.encode(wire)
        }]
    })
}

fn legacy_transfer_wire(payer: &Pubkey) -> Vec<u8> {
    let mut message = solana_message::Message::new(
        &[solana_system_interface::instruction::transfer(
            payer,
            &Pubkey::new_from_array([2; 32]),
            1,
        )],
        Some(payer),
    );
    message.recent_blockhash = solana_hash::Hash::new_from_array([1; 32]);
    let nsig = usize::from(message.header.num_required_signatures);
    wincode::serialize(&solana_transaction::versioned::VersionedTransaction {
        signatures: vec![solana_signature::Signature::default(); nsig],
        message: solana_message::VersionedMessage::Legacy(message),
    })
    .unwrap()
}

fn v0_transfer_wire(payer: &Pubkey) -> Vec<u8> {
    let header = solana_message::MessageHeader {
        num_required_signatures: 1,
        num_readonly_signed_accounts: 0,
        num_readonly_unsigned_accounts: 1,
    };
    let data = {
        let mut data = 2u32.to_le_bytes().to_vec();
        data.extend_from_slice(&1u64.to_le_bytes());
        data
    };
    let message = solana_message::v0::Message {
        header,
        account_keys: vec![
            *payer,
            wallet_core::SYSTEM_PROGRAM_ID,
            Pubkey::new_from_array([2; 32]),
        ],
        recent_blockhash: solana_hash::Hash::new_from_array([1; 32]),
        instructions: vec![solana_message::compiled_instruction::CompiledInstruction {
            program_id_index: 1,
            accounts: vec![0, 2],
            data,
        }],
        address_table_lookups: vec![],
    };
    wincode::serialize(&solana_transaction::versioned::VersionedTransaction {
        signatures: vec![solana_signature::Signature::default()],
        message: solana_message::VersionedMessage::V0(message),
    })
    .unwrap()
}

#[tokio::test]
async fn a_site_never_connected_cannot_ask_for_a_signature() {
    let (kit, bridge) = attached_bridge();
    let value = ask(&bridge, sign_request("stranger", 1));
    assert_eq!(value["error"]["code"], "rejected");
    assert!(kit.core.slices().bridge.get().is_empty());
}

#[tokio::test]
async fn a_connected_site_cannot_sign_for_another_account() {
    let (kit, bridge) = attached_bridge();
    connected(&kit, &bridge, false);
    let mut request = sign_request("other", 1);
    request["params"]["transactions"][0]["account"] =
        json!(Pubkey::new_from_array([9; 32]).to_string());
    assert_eq!(ask(&bridge, request)["error"]["code"], "rejected");

    let mut unnamed = sign_request("unnamed", 2);
    unnamed["params"]["transactions"][1]
        .as_object_mut()
        .unwrap()
        .remove("account");
    assert_eq!(ask(&bridge, unnamed)["error"]["code"], "rejected");
    assert!(kit.core.slices().bridge.get().is_empty());
}

#[tokio::test]
async fn a_silent_connect_never_opens_the_modal() {
    let (kit, bridge) = attached_bridge();
    let value = silent_connect(&bridge);
    assert_eq!(value["result"]["accounts"], json!([]));
    assert!(kit.core.slices().bridge.get().is_empty());
}

#[test]
fn a_silent_connect_gets_no_error_while_the_app_starts() {
    let value = silent_connect(&Bridge::new());
    assert_eq!(value["result"]["accounts"], json!([]));
}

#[tokio::test]
async fn a_disconnected_site_is_not_let_back_in_silently_or_to_sign() {
    let (kit, bridge) = attached_bridge();
    connected(&kit, &bridge, true);
    ask(
        &bridge,
        json!({ "v": 1, "id": "d", "origin": ORIGIN, "method": "disconnect" }),
    );
    assert_eq!(silent_connect(&bridge)["result"]["accounts"], json!([]));
    assert_eq!(
        ask(&bridge, sign_request("after", 1))["error"]["code"],
        "rejected"
    );
    // The grant itself stays.
    let grant = kit
        .core
        .with_workspace(|workspace| workspace.connections.grant(ORIGIN).cloned());
    assert_eq!(grant.unwrap().accounts, vec![account()]);
}

#[tokio::test]
async fn a_blocked_site_is_turned_away_without_the_modal() {
    let (kit, bridge) = attached_bridge();
    let asking = {
        let bridge = bridge.clone();
        std::thread::spawn(move || {
            ask(
                &bridge,
                json!({ "v": 1, "id": "first", "origin": ORIGIN, "method": "connect" }),
            )
        })
    };
    let second = {
        let bridge = bridge.clone();
        std::thread::spawn(move || {
            ask(
                &bridge,
                json!({ "v": 1, "id": "second", "origin": ORIGIN, "method": "connect" }),
            )
        })
    };
    while kit.core.slices().bridge.get().len() < 2 {
        std::thread::yield_now();
    }
    assert!(kit.core.bridge_block("first"));
    assert_eq!(asking.join().unwrap()["error"]["code"], "rejected");
    assert_eq!(second.join().unwrap()["error"]["code"], "rejected");
    assert!(kit.core.slices().bridge.get().is_empty());

    let value = ask(
        &bridge,
        json!({ "v": 1, "id": "again", "origin": ORIGIN, "method": "connect" }),
    );
    assert_eq!(value["error"]["code"], "rejected");
    assert_eq!(silent_connect(&bridge)["result"]["accounts"], json!([]));
    assert_eq!(
        ask(&bridge, sign_request("blocked", 1))["error"]["code"],
        "rejected"
    );
    assert!(kit.core.slices().bridge.get().is_empty());
}

fn held(pairs: &[(&str, &[u8])]) -> BTreeMap<String, Vec<Pubkey>> {
    pairs
        .iter()
        .map(|(origin, keys)| {
            let keys = keys
                .iter()
                .map(|b| Pubkey::new_from_array([*b; 32]))
                .collect();
            ((*origin).to_owned(), keys)
        })
        .collect()
}

fn told(frames: Vec<Vec<u8>>) -> Vec<Value> {
    frames
        .iter()
        .map(|frame| serde_json::from_slice(frame).unwrap())
        .collect()
}

#[tokio::test]
async fn a_site_is_told_only_when_what_it_holds_changes() {
    let (kit, _bridge) = attached_bridge();
    let core = kit.core.public();
    let before = held(&[("https://a.test", &[1, 2]), ("https://b.test", &[3])]);

    // The same, or a site that just got its accounts in a reply.
    let after = held(&[
        ("https://a.test", &[1, 2]),
        ("https://b.test", &[3]),
        ("https://c.test", &[4]),
    ]);
    assert!(changes(&before, &after, &core).is_empty());

    // A new first account, and a site that holds nothing now.
    let after = held(&[("https://a.test", &[2, 1])]);
    let frames = told(changes(&before, &after, &core));
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["method"], "accountsChanged");
    assert_eq!(frames[0]["origin"], "https://a.test");
    assert_eq!(frames[0]["params"]["accounts"].as_array().unwrap().len(), 2);
    assert_eq!(frames[1]["origin"], "https://b.test");
    assert_eq!(frames[1]["params"]["accounts"], json!([]));
}

#[tokio::test]
async fn a_request_waiting_for_the_user_holds_up_nothing_else() {
    let (kit, bridge) = attached_bridge();
    let peer = Peer::new(bridge.clone(), None);
    let waiting = sent(
        &peer,
        json!({ "v": 1, "id": "w", "origin": ORIGIN, "method": "connect" }),
    );
    assert!(matches!(waiting, Answer::Later(_)));
    assert_eq!(kit.core.slices().bridge.get().len(), 1);

    let answer = sent(
        &peer,
        json!({ "v": 1, "id": "s", "origin": ORIGIN, "method": "connect", "params": { "silent": true } }),
    );
    assert!(matches!(answer, Answer::Now(_)));
    assert_eq!(read(&settled(answer))["result"]["accounts"], json!([]));

    assert!(kit.core.bridge_answer("w", &BridgeAnswer::Rejected));
    assert_eq!(read(&settled(waiting))["error"]["code"], "rejected");
}

fn connect_from(id: &str, origin: &str) -> Value {
    json!({ "v": 1, "id": id, "origin": origin, "method": "connect" })
}

fn cancel_of(id: &str) -> Value {
    json!({ "v": 1, "id": "c", "method": "cancel", "params": { "id": id } })
}

#[tokio::test]
async fn a_request_its_page_gave_up_on_leaves_the_queue() {
    let (kit, bridge) = attached_bridge();
    let peer = Peer::new(bridge.clone(), None);
    let waiting = sent(&peer, connect_from("w", ORIGIN));
    assert_eq!(kit.core.slices().bridge.get().len(), 1);

    // The cancel follows its request straight away, and still finds it.
    let reply = read(&settled(sent(&peer, cancel_of("w"))));
    assert_eq!(reply["id"], "c");
    assert!(reply.get("error").is_none());
    assert!(kit.core.slices().bridge.get().is_empty());
    assert_eq!(read(&settled(waiting))["error"]["code"], "expired");
    assert!(peer.waiting.lock().unwrap().is_empty());
}

#[tokio::test]
async fn one_browser_cannot_cancel_what_another_asked() {
    let (kit, bridge) = attached_bridge();
    let chrome = Peer::new(bridge.clone(), None);
    let edge = Peer::new(bridge.clone(), None);
    let waiting = sent(&chrome, connect_from("w", ORIGIN));

    settled(sent(&edge, cancel_of("w")));
    assert_eq!(kit.core.slices().bridge.get().len(), 1);

    assert!(kit.core.bridge_answer("w", &BridgeAnswer::Rejected));
    assert_eq!(read(&settled(waiting))["error"]["code"], "rejected");
}

#[tokio::test]
async fn a_browser_going_away_ends_only_its_own_requests() {
    let (kit, bridge) = attached_bridge();
    let chrome = Peer::new(bridge.clone(), None);
    let edge = Peer::new(bridge.clone(), None);
    let gone = sent(&chrome, connect_from("gone", ORIGIN));
    let stays = sent(&edge, connect_from("stays", ORIGIN));

    chrome.closed();
    let queue = kit.core.slices().bridge.get();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.head().unwrap().id, "stays");
    assert_eq!(read(&settled(gone))["error"]["code"], "expired");

    assert!(kit.core.bridge_answer("stays", &BridgeAnswer::Rejected));
    assert_eq!(read(&settled(stays))["error"]["code"], "rejected");
}

#[tokio::test]
async fn a_send_the_user_approved_goes_on_without_the_browser() {
    let (kit, bridge) = attached_bridge();
    connected(&kit, &bridge, false);
    let peer = Peer::new(bridge.clone(), None);
    let waiting = sent(&peer, send_request("send", SEND, Some("solana:devnet")));
    let sending = kit.core.bridge_begin_send("send", 1).unwrap();
    assert!(kit.core.slices().bridge.get().is_empty());

    peer.closed();
    kit.core.bridge_send_finished(sending, vec![Ok([1u8; 64])]);
    let reply = read(&settled(waiting));
    assert_eq!(reply["id"], "send");
    assert_eq!(reply["result"]["signatures"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn the_queue_turns_away_a_flood_and_answers_it_at_once() {
    let (kit, bridge) = attached_bridge();
    let peer = Peer::new(bridge.clone(), None);
    let mut waits = Vec::new();
    for n in 0..bridge_protocol::MAX_PENDING_PER_ORIGIN {
        let answer = sent(&peer, connect_from(&format!("w{n}"), ORIGIN));
        assert!(matches!(answer, Answer::Later(_)));
        waits.push(answer);
    }
    let refused = sent(&peer, connect_from("over", ORIGIN));
    assert!(matches!(refused, Answer::Now(_)));
    assert_eq!(
        read(&settled(refused))["error"]["code"],
        "too_many_requests"
    );

    // Another site still gets through.
    let other = sent(&peer, connect_from("other", "https://other.test"));
    assert!(matches!(other, Answer::Later(_)));
    waits.push(other);

    // An id that is waiting is not taken twice: the first would lose its reply.
    let again = read(&settled(sent(
        &peer,
        connect_from("w0", "https://other.test"),
    )));
    assert_eq!(again["error"]["code"], "bad_request");

    peer.closed();
    assert!(kit.core.slices().bridge.get().is_empty());
    for wait in waits {
        assert_eq!(read(&settled(wait))["error"]["code"], "expired");
    }
}

#[tokio::test]
async fn the_pages_a_browser_lists_decide_who_shows_as_connected() {
    let (kit, bridge) = attached_bridge();
    connected(&kit, &bridge, true);
    let sites = || kit.core.slices().sites.get();
    assert!(sites().connected.is_empty());
    assert!(sites().holding.contains_key(ORIGIN));

    kit.core
        .public()
        .site_pages(BTreeSet::from([ORIGIN.to_string()]));
    assert_eq!(sites().connected[0].origin, ORIGIN);
    assert!(sites().connected[0].remembered);

    kit.core.public().site_pages(BTreeSet::new());
    assert!(sites().connected.is_empty());
    assert!(!sites().holding.contains_key(ORIGIN));
    assert_eq!(sites().authorized.len(), 1);
}

#[tokio::test]
async fn disconnecting_keeps_the_grant_and_revoking_does_not() {
    let (kit, bridge) = attached_bridge();
    connected(&kit, &bridge, true);
    kit.core.site_disconnect(ORIGIN);
    let sites = kit.core.slices().sites.get();
    assert!(!sites.holding.contains_key(ORIGIN));
    assert_eq!(sites.authorized.len(), 1);
    assert_eq!(
        ask(&bridge, sign_request("after", 1))["error"]["code"],
        "rejected"
    );

    kit.core.site_revoke(ORIGIN);
    assert!(kit.core.slices().sites.get().authorized.is_empty());
}

#[tokio::test]
async fn unblocking_lets_a_site_ask_again() {
    let (kit, bridge) = attached_bridge();
    kit.core.site_block(ORIGIN);
    assert_eq!(kit.core.slices().sites.get().blocked, vec![ORIGIN]);
    kit.core.site_unblock(ORIGIN);
    assert!(kit.core.slices().sites.get().blocked.is_empty());
    connected(&kit, &bridge, false);
}

fn sign_message(id: &str, account: Pubkey, message: &[u8]) -> Value {
    json!({
        "v": 1, "id": id, "origin": ORIGIN, "method": "signMessage",
        "params": {
            "account": account.to_string(),
            "message": base64::engine::general_purpose::STANDARD.encode(message),
        }
    })
}

#[tokio::test]
async fn a_message_needs_a_connected_account_with_a_key_here() {
    let (kit, bridge) = attached_bridge();
    assert_eq!(
        ask(&bridge, sign_message("stranger", account(), b"hello"))["error"]["code"],
        "rejected"
    );
    // Connected, but this test core holds no key for it.
    connected(&kit, &bridge, false);
    let value = ask(&bridge, sign_message("keyless", account(), b"hello"));
    assert_eq!(value["error"]["code"], "rejected");
    assert!(value["error"]["message"]
        .as_str()
        .unwrap()
        .contains("No key"));
    assert!(kit.core.slices().bridge.get().is_empty());
}

fn sign_in_ask(id: &str, params: Value) -> Value {
    json!({ "v": 1, "id": id, "origin": ORIGIN, "method": "signIn", "params": params })
}

#[tokio::test]
async fn a_sign_in_that_cannot_be_honest_never_reaches_the_user() {
    let (kit, bridge) = attached_bridge();
    let other = ask(
        &bridge,
        sign_in_ask("o", json!({ "domain": "bank.example" })),
    );
    assert!(other["error"]["message"]
        .as_str()
        .unwrap()
        .contains("bank.example"));
    let injected = ask(
        &bridge,
        sign_in_ask("i", json!({ "statement": "a\nURI: b" })),
    );
    assert_eq!(injected["error"]["code"], "rejected");
    let offchain = ask(
        &bridge,
        sign_in_ask(
            "c",
            json!({ "useOffchainMessage": { "messageVersion": 1 } }),
        ),
    );
    assert_eq!(offchain["error"]["code"], "rejected");
    let keyless = ask(
        &bridge,
        sign_in_ask("k", json!({ "address": account().to_string() })),
    );
    assert_eq!(keyless["error"]["code"], "rejected");
    kit.core.site_block(ORIGIN);
    let blocked = ask(&bridge, sign_in_ask("b", json!({})));
    assert_eq!(blocked["error"]["code"], "rejected");
    assert!(kit.core.slices().bridge.get().is_empty());
}

#[tokio::test]
async fn a_sign_in_asking_nothing_reaches_the_user() {
    let (kit, bridge) = attached_bridge();
    let asking = {
        let bridge = bridge.clone();
        std::thread::spawn(move || ask(&bridge, sign_in_ask("s", Value::Null)))
    };
    while kit.core.slices().bridge.get().is_empty() {
        std::thread::yield_now();
    }
    assert_eq!(
        BridgePrompt::head_of(&kit.core.slices().bridge.get())
            .unwrap()
            .method,
        "signIn"
    );
    assert!(kit.core.bridge_answer("s", &BridgeAnswer::Rejected));
    asking.join().unwrap();
}

fn send_request(id: &str, method: &str, chain: Option<&str>) -> Value {
    let mut request = sign_request(id, 1);
    request["method"] = json!(method);
    if let Some(chain) = chain {
        request["params"]["transactions"][0]["chain"] = json!(chain);
    }
    request
}

#[tokio::test]
async fn a_send_names_its_network_and_a_connected_account() {
    let (kit, bridge) = attached_bridge();
    let stranger = ask(&bridge, send_request("s", SEND, Some("solana:devnet")));
    assert_eq!(stranger["error"]["code"], "rejected");
    let unnamed = ask(&bridge, send_request("u", SEND, None));
    assert_eq!(unnamed["error"]["code"], "bad_request");
    let mut empty = send_request("e", SEND_ALL, Some("solana:devnet"));
    empty["params"]["transactions"] = json!([]);
    assert_eq!(ask(&bridge, empty)["error"]["code"], "bad_request");
    assert!(kit.core.slices().bridge.get().is_empty());
}

/// A send waiting for the user, on its own thread.
fn waiting_send(
    kit: &Kit,
    bridge: &Bridge,
    method: &'static str,
) -> std::thread::JoinHandle<Value> {
    connected(kit, bridge, false);
    let asking = {
        let bridge = bridge.clone();
        std::thread::spawn(move || {
            ask(&bridge, send_request("send", method, Some("solana:devnet")))
        })
    };
    while kit.core.slices().bridge.get().is_empty() {
        std::thread::yield_now();
    }
    asking
}

#[tokio::test]
async fn an_approved_send_leaves_the_queue_and_answers_when_done() {
    let (kit, bridge) = attached_bridge();
    let asking = waiting_send(&kit, &bridge, SEND);
    let shown = BridgePrompt::head_of(&kit.core.slices().bridge.get()).unwrap();
    assert!(shown.sends);
    assert_eq!(shown.chains, ["solana:devnet"]);

    // A signed transaction is not what a send answers with.
    assert!(!kit
        .core
        .bridge_answer("send", &BridgeAnswer::Signed(vec![vec![0x81]])));
    let sending = kit.core.bridge_begin_send("send", 1).unwrap();
    assert!(kit.core.slices().bridge.get().is_empty());
    // Nobody can reject it now.
    assert!(!kit.core.bridge_answer("send", &BridgeAnswer::Rejected));

    kit.core.bridge_send_finished(sending, vec![Ok([1; 64])]);
    let value = asking.join().unwrap();
    assert_eq!(value["result"]["signatures"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_failed_send_tells_the_site_why() {
    let (kit, bridge) = attached_bridge();
    let asking = waiting_send(&kit, &bridge, SEND);
    let sending = kit.core.bridge_begin_send("send", 1).unwrap();
    kit.core.bridge_send_finished(
        sending,
        vec![Err(SendError::Failed("the node said no".into()))],
    );
    let value = asking.join().unwrap();
    assert_eq!(value["error"]["code"], "send_failed");
    assert_eq!(value["error"]["message"], "the node said no");
}

#[tokio::test]
async fn a_send_that_may_still_land_is_not_called_a_failure() {
    let (kit, bridge) = attached_bridge();
    let asking = waiting_send(&kit, &bridge, SEND);
    let sending = kit.core.bridge_begin_send("send", 1).unwrap();
    kit.core.bridge_send_finished(
        sending,
        vec![Err(SendError::Unknown("it may still land".into()))],
    );
    let value = asking.join().unwrap();
    assert_eq!(value["error"]["code"], "send_unknown");
    assert_eq!(value["error"]["message"], "it may still land");
}

#[tokio::test]
async fn a_send_approved_as_time_runs_out_answers_with_its_result() {
    let (kit, bridge) = attached_bridge();
    connected(&kit, &bridge, false);
    let public = kit.core.public();
    let request = BridgeRequest {
        id: "late".into(),
        origin: ORIGIN.into(),
        method: SEND.into(),
        params: send_request("late", SEND, Some("solana:devnet"))["params"].clone(),
    };
    let waiting = wait_for_user(
        &public,
        request,
        Duration::from_millis(50),
        &Waiting::default(),
    );
    let sending = kit.core.bridge_begin_send("late", 1).unwrap();

    // The time runs out with the send still going.
    let answer = std::thread::spawn(move || read(&settled(waiting)));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!answer.is_finished());

    kit.core.bridge_send_finished(sending, vec![Ok([1; 64])]);
    let reply = answer.join().unwrap();
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(reply["result"]["signatures"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_send_dropped_without_a_result_does_not_wait_for_ever() {
    let (kit, bridge) = attached_bridge();
    connected(&kit, &bridge, false);
    let request = BridgeRequest {
        id: "lost".into(),
        origin: ORIGIN.into(),
        method: SEND.into(),
        params: send_request("lost", SEND, Some("solana:devnet"))["params"].clone(),
    };
    let waiting = wait_for_user(
        &kit.core.public(),
        request,
        Duration::from_millis(10),
        &Waiting::default(),
    );
    drop(kit.core.bridge_begin_send("lost", 1).unwrap());
    assert_eq!(read(&settled(waiting))["error"]["code"], "expired");
}

#[tokio::test]
async fn send_all_answers_for_each_transaction() {
    let (kit, bridge) = attached_bridge();
    let asking = waiting_send(&kit, &bridge, SEND_ALL);
    assert!(kit.core.bridge_begin_send("send", 2).is_none());
    let sending = kit.core.bridge_begin_send("send", 1).unwrap();
    kit.core
        .bridge_send_finished(sending, vec![Err(SendError::Failed("failed".into()))]);
    let value = asking.join().unwrap();
    assert_eq!(value["result"]["results"][0]["error"]["message"], "failed");
}

#[tokio::test]
async fn disconnecting_rejects_a_waiting_message_as_well() {
    let (kit, bridge) = attached_bridge();
    let asking = waiting_send(&kit, &bridge, SEND);
    kit.core.site_disconnect(ORIGIN);
    assert_eq!(asking.join().unwrap()["error"]["code"], "rejected");
    assert!(kit.core.slices().bridge.get().is_empty());
}

#[test]
fn a_readable_transaction_wire_is_accepted() {
    let payer = wallet_core::KeypairSigner::new_random();
    let v1 = wallet_core::compile_v1(
        payer.pubkey(),
        vec![solana_system_interface::instruction::transfer(
            &payer.pubkey(),
            &Pubkey::new_from_array([2; 32]),
            1,
        )],
        None,
        Some(solana_hash::Hash::new_from_array([1; 32])),
    )
    .unwrap()
    .wire;
    assert_eq!(transaction_wire(&params_for_wire(&v1), 0).unwrap(), v1);

    let legacy = legacy_transfer_wire(&payer.pubkey());
    assert_eq!(
        transaction_wire(&params_for_wire(&legacy), 0).unwrap(),
        legacy
    );
    assert!(sign_detail(&params_for_wire(&legacy), 0).contains("legacy"));

    let v0 = v0_transfer_wire(&payer.pubkey());
    assert_eq!(transaction_wire(&params_for_wire(&v0), 0).unwrap(), v0);
    assert!(sign_detail(&params_for_wire(&v0), 0).contains("v0"));

    let garbage = base64::engine::general_purpose::STANDARD.encode([0xff, 1, 2]);
    let params =
        json!({ "transactions": [{ "transaction": v1_b64(&v1) }, { "transaction": garbage }] });
    assert_eq!(transaction_wire(&params, 0).unwrap(), v1);
    assert!(transaction_wire(&params, 1).is_none());
    assert!(transaction_wire(&params, 2).is_none());
}

#[test]
fn a_connection_counts_as_the_extension_until_it_closes() {
    let bridge = Bridge::new();
    let extension = bridge.subscribe_extension();
    let status = serde_json::to_vec(&json!({ "v": 1, "id": "s", "method": "status" })).unwrap();

    let first = Peer::new(bridge.clone(), None);
    settled(first.answer(&status));
    settled(first.answer(&status));
    let second = Peer::new(bridge.clone(), None);
    settled(second.answer(&status));
    assert_eq!(*extension.borrow(), 2);

    first.closed();
    assert_eq!(*extension.borrow(), 1);
    second.closed();
    assert_eq!(*extension.borrow(), 0);
}

#[test]
fn a_second_launch_asking_to_show_is_not_the_extension() {
    let bridge = Bridge::new();
    let extension = bridge.subscribe_extension();
    let launch = Peer::new(bridge.clone(), None);
    settled(launch.answer(br#"{"v":1,"id":"show","method":"show"}"#));
    assert_eq!(*extension.borrow(), 0);
    drop(launch);
    assert_eq!(*extension.borrow(), 0);
}
