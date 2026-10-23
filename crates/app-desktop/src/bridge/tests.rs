use super::*;
use app_core::bridge::BridgeAnswer;
use app_core::testing::{kit, Kit};
use app_core::{BridgeRequestOutcome, NoticeKind};
use wallet_core::WalletSigner;

/// A bridge attached to a test core.
fn attached_bridge() -> (Kit, Bridge) {
    let kit = kit();
    let bridge = Bridge::new();
    bridge.attach(kit.core.public());
    (kit, bridge)
}

fn ask(bridge: &Bridge, request: Value) -> Value {
    serde_json::from_slice(&bridge.answer(&serde_json::to_vec(&request).unwrap())).unwrap()
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
    let bytes = wait_for_user(
        &kit.core.public(),
        connect("stale"),
        Duration::from_millis(10),
    );
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
    assert_eq!(requested_account(&params), Some(account));
    assert_eq!(requested_account(&json!({ "transactions": [{}] })), None);
    assert_eq!(
        requested_account(&json!({ "transactions": [{ "account": "not an address" }] })),
        None
    );

    let (kit, _bridge) = attached_bridge();
    let waiting = |id: &'static str| {
        let core = kit.core.public();
        std::thread::spawn(move || wait_for_user(&core, connect(id), Duration::from_secs(5)))
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
        &BridgeAnswer::Connect(vec![([1; 32], String::new())])
    ));
    assert!(kit.core.bridge_answer("count-2", &BridgeAnswer::Rejected));
    let first: Value = serde_json::from_slice(&first.join().unwrap()).unwrap();
    let second: Value = serde_json::from_slice(&second.join().unwrap()).unwrap();
    assert_eq!(first["id"], "count-1");
    assert_eq!(first["result"]["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(second["error"]["code"], "rejected");
    assert!(BridgePrompt::head_of(&kit.core.slices().bridge.get()).is_none());
}

#[tokio::test]
async fn a_batch_of_transactions_is_refused() {
    let (kit, bridge) = attached_bridge();
    let v1 = base64::engine::general_purpose::STANDARD.encode([0x81, 1, 2]);
    let value = ask(
        &bridge,
        json!({
            "v": 1,
            "id": "batch",
            "origin": "https://example.test",
            "method": "signTransaction",
            "params": { "transactions": [{ "transaction": v1 }, { "transaction": v1 }] }
        }),
    );
    assert_eq!(value["error"]["code"], "not_implemented");
    assert!(kit.core.slices().bridge.get().is_empty());
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
    assert_eq!(transaction_wire(&params_for_wire(&v1)).unwrap(), v1);

    let legacy = legacy_transfer_wire(&payer.pubkey());
    assert_eq!(transaction_wire(&params_for_wire(&legacy)).unwrap(), legacy);
    assert!(sign_detail(&params_for_wire(&legacy)).contains("legacy"));

    let v0 = v0_transfer_wire(&payer.pubkey());
    assert_eq!(transaction_wire(&params_for_wire(&v0)).unwrap(), v0);
    assert!(sign_detail(&params_for_wire(&v0)).contains("v0"));

    let garbage = base64::engine::general_purpose::STANDARD.encode([0xff, 1, 2]);
    let params = json!({ "transactions": [{ "transaction": garbage }] });
    assert!(transaction_wire(&params).is_none());
}

#[test]
fn a_connection_counts_as_the_extension_until_it_closes() {
    let bridge = Bridge::new();
    let extension = bridge.subscribe_extension();
    let status = serde_json::to_vec(&json!({ "v": 1, "id": "s", "method": "status" })).unwrap();

    let mut first = Peer::new(bridge.clone());
    first.answer(&status);
    first.answer(&status);
    let mut second = Peer::new(bridge.clone());
    second.answer(&status);
    assert_eq!(*extension.borrow(), 2);

    drop(first);
    assert_eq!(*extension.borrow(), 1);
    drop(second);
    assert_eq!(*extension.borrow(), 0);
}

#[test]
fn a_second_launch_asking_to_show_is_not_the_extension() {
    let bridge = Bridge::new();
    let extension = bridge.subscribe_extension();
    let mut launch = Peer::new(bridge.clone());
    launch.answer(br#"{"v":1,"id":"show","method":"show"}"#);
    assert_eq!(*extension.borrow(), 0);
    drop(launch);
    assert_eq!(*extension.borrow(), 0);
}
