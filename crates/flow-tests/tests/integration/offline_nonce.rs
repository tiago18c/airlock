//! An offline key on its durable nonce: what it signs for itself stays
//! valid however long the trip to the other device takes, and lands once.
//! Deprecated once the programmatic signer or Vector is on mainnet.

use crate::support::node::Node;
use crate::support::{Chain, SOL};
use solana_pubkey::Pubkey;
use std::sync::Mutex;
use wallet_core::{
    apply_local_signatures, decode_v1_transaction, signing_route, sol_transfer, KeypairSigner,
    WalletRecord, WalletSigner,
};

/// A hot wallet that pays, and an offline key whose signer is in the test.
struct Desk {
    node: Node,
    wallets: Vec<WalletRecord>,
    payer: KeypairSigner,
    cold: KeypairSigner,
}

impl Desk {
    fn new() -> Self {
        let mut chain = Chain::new();
        let payer = KeypairSigner::new_random();
        let cold = KeypairSigner::new_random();
        chain.fund(&payer.pubkey(), 10 * SOL);
        chain.fund(&cold.pubkey(), 2 * SOL);
        let hot = WalletRecord::from_keypair_seed("Fees", payer.seed_bytes());
        let mut offline = WalletRecord::offline("Cold", cold.pubkey());
        offline.kind.set_fee_payer(Some(hot.id));
        Self {
            node: Node(Mutex::new(chain)),
            wallets: vec![hot, offline],
            payer,
            cold,
        }
    }

    fn route(&self) -> wallet_core::SigningRoute {
        signing_route(&self.wallets, self.wallets[1].id).expect("a route")
    }

    /// Create the key's nonce the way the row does, and remember it.
    fn create_nonce(&mut self) -> Pubkey {
        let made = pollster::block_on(wallet_flows::offline_nonce::create(
            &self.node,
            &self.route(),
        ))
        .expect("prepares");
        let signed = apply_local_signatures(&made.wire, &[&self.payer as &dyn WalletSigner])
            .expect("the payer alone signs");
        self.node.chain().send_wire(&signed).expect("lands");
        self.wallets[1].kind.set_durable_nonce(Some(made.account));
        made.account
    }

    fn sign_both(&self, wire: &[u8]) -> Vec<u8> {
        apply_local_signatures(wire, &[&self.payer as &dyn WalletSigner, &self.cold])
            .expect("signs")
    }

    /// Let more blocks pass than a blockhash is accepted for.
    fn wait_long(&self) {
        self.node.chain().age_blockhashes(200);
    }
}

#[test]
fn a_send_on_the_nonce_outlives_a_blockhash_and_lands_once() {
    let mut desk = Desk::new();
    let to = Pubkey::new_unique();

    // On a blockhash, the trip to the offline device is too long.
    let (late, height) = pollster::block_on(wallet_flows::transfer::prepare(
        &desk.node,
        &desk.route(),
        to,
        SOL,
    ))
    .unwrap();
    assert!(height.is_some());

    let nonce = desk.create_nonce();
    let route = desk.route();
    assert_eq!(route.durable_nonce, Some(nonce));
    let (first, height) =
        pollster::block_on(wallet_flows::transfer::prepare(&desk.node, &route, to, SOL)).unwrap();
    assert_eq!(height, None);
    let (second, _) =
        pollster::block_on(wallet_flows::transfer::prepare(&desk.node, &route, to, SOL)).unwrap();
    assert!(
        decode_v1_transaction(&first.wire)
            .unwrap()
            .uses_durable_nonce
    );

    desk.wait_long();
    assert!(desk
        .node
        .chain()
        .send_wire(&desk.sign_both(&late.wire))
        .is_err());
    desk.node
        .chain()
        .send_wire(&desk.sign_both(&first.wire))
        .expect("the nonce kept it valid");
    assert_eq!(desk.node.chain().balance(&to), SOL);

    // The nonce advanced, so the other one made on it is void.
    assert!(desk
        .node
        .chain()
        .send_wire(&desk.sign_both(&second.wire))
        .is_err());
    assert_eq!(desk.node.chain().balance(&to), SOL);
}

#[test]
fn a_site_request_for_the_key_moves_onto_its_nonce_and_lands_after_the_wait() {
    let mut desk = Desk::new();
    let nonce = desk.create_nonce();
    let to = Pubkey::new_unique();
    let blockhash = desk.node.chain().latest_blockhash();
    let site = wallet_core::compile_v1(
        desk.payer.pubkey(),
        vec![sol_transfer(&desk.cold.pubkey(), &to, SOL / 2)],
        None,
        Some(blockhash),
    )
    .unwrap()
    .wire;
    let tx = decode_v1_transaction(&site).unwrap();
    let moved = pollster::block_on(wallet_flows::offline_nonce::move_onto(
        &desk.node,
        &tx,
        desk.cold.pubkey(),
        nonce,
    ))
    .unwrap();

    desk.wait_long();
    desk.node
        .chain()
        .send_wire(&desk.sign_both(&moved))
        .expect("lands on the nonce");
    assert_eq!(desk.node.chain().balance(&to), SOL / 2);
}
