//! What the Sign panel does to a transaction, against the real programs.
//!
//! Build compiles an ordinary transaction in which a programmatic signer's
//! address is a required signer. No key can sign for that address. The panel
//! rebuilds the transaction around a `Submit`, the authority signs the
//! authorization (here on a second device, returning only a signature), the
//! hot wallet signs the result, and the exact bytes go to the chain.

use crate::support::node::Node;
use crate::support::{carried, logs, Chain, SOL};
use std::sync::Mutex;
use wallet_core::programmatic::{create_executor_nonce, programmatic_signer_address};
use wallet_core::protocol::QrObject;
use wallet_core::relay::{
    accept_signature, authority_signature_objects, authority_slots, programmatic_signers_of,
    sign_authorizations, with_blockhash, wrap_for_programmatic, SignatureUse,
};
use wallet_core::squads::{Squad, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL};
use wallet_core::{
    apply_local_signatures, compile_v1, decode_v1_transaction, next_step, signing_status,
    Instruction, KeypairSigner, NextSignature, WalletSigner,
};

struct Desk {
    chain: Chain,
    hot: KeypairSigner,
    authority: KeypairSigner,
    /// The authority's programmatic signer address.
    address: solana_pubkey::Pubkey,
    nonce: solana_pubkey::Pubkey,
}

impl Desk {
    fn new() -> Self {
        let mut chain = Chain::new();
        let hot = KeypairSigner::new_random();
        let authority = KeypairSigner::new_random();
        let address = programmatic_signer_address(&authority.pubkey());
        chain.fund(&hot.pubkey(), 10 * SOL);
        let (nonce, create) = create_executor_nonce(
            &hot.pubkey(),
            &hot.pubkey(),
            "airlock",
            &address,
            chain.nonce_rent(),
        )
        .unwrap();
        chain.send(&[&hot], create.to_vec()).unwrap();
        Self {
            chain,
            hot,
            authority,
            address,
            nonce,
        }
    }

    /// What Build hands over: the hot wallet pays, the instructions name the
    /// programmatic signer's address wherever it has to sign.
    fn built(&self, instructions: Vec<Instruction>) -> Vec<u8> {
        compile_v1(
            self.hot.pubkey(),
            instructions,
            None,
            Some(self.chain.latest_blockhash()),
        )
        .unwrap()
        .wire
    }

    /// The panel's one change to a transaction.
    fn wrap(&self, built: &[u8]) -> Vec<u8> {
        let tx = decode_v1_transaction(built).unwrap();
        assert_eq!(
            programmatic_signers_of(&tx, &[self.authority.pubkey()]),
            vec![(self.authority.pubkey(), self.address)]
        );
        wrap_for_programmatic(
            &tx,
            &[self.authority.pubkey()],
            self.chain.nonce(&self.nonce),
            self.hot.pubkey(),
            self.chain.latest_blockhash(),
        )
        .unwrap()
    }

    /// The device that holds the authority key: it loads the transaction,
    /// signs the authorization and shows one signature.
    fn cold_signature(&self, wrapped: &[u8]) -> QrObject {
        let signed = sign_authorizations(wrapped, &[&self.authority as &dyn WalletSigner]).unwrap();
        let objects = authority_signature_objects(&decode_v1_transaction(&signed.wire).unwrap());
        assert_eq!(objects.len(), 1);
        carried(&objects[0].signed)
    }

    fn hot_signs(&self, wire: &[u8]) -> Vec<u8> {
        apply_local_signatures(wire, &[&self.hot as &dyn WalletSigner]).unwrap()
    }
}

fn pay(from: &solana_pubkey::Pubkey, to: &solana_pubkey::Pubkey, lamports: u64) -> Instruction {
    solana_system_interface::instruction::transfer(from, to, lamports)
}

#[test]
fn a_built_transaction_is_wrapped_signed_on_two_devices_and_lands() {
    let mut desk = Desk::new();
    let recipient = KeypairSigner::new_random().pubkey();
    desk.chain.fund(&desk.address, SOL);

    let built = desk.built(vec![pay(&desk.address, &recipient, SOL / 2)]);
    let wrapped = desk.wrap(&built);

    // Signed by the payer alone it reaches the chain and is refused there:
    // the authority's place is still empty.
    let unsigned = desk.hot_signs(&wrapped);
    let failure = desk.chain.send_wire(&unsigned).unwrap_err();
    assert!(
        logs(&failure).contains("Custom"),
        "an empty authorization should be refused by the signer program: {}",
        logs(&failure)
    );
    assert_eq!(desk.chain.balance(&recipient), 0);

    // The authority's signature arrives saying it signs an authorization.
    let QrObject::Signature(signed) = desk.cold_signature(&wrapped) else {
        panic!("a signature object");
    };
    let (authorized, used) = accept_signature(&wrapped, &signed).unwrap();
    assert_eq!(used, SignatureUse::Authorization { dropped: 0 });
    let tx = decode_v1_transaction(&authorized).unwrap();
    assert!(authority_slots(&tx).iter().all(|slot| slot.signed));
    let before = signing_status(
        &decode_v1_transaction(&wrapped).unwrap(),
        &[desk.authority.pubkey()],
    );
    assert_eq!(next_step(&before), NextSignature::Authorization);
    let after = signing_status(&tx, &[desk.hot.pubkey()]);
    assert_eq!(next_step(&after), NextSignature::Transaction);
    let done = desk.hot_signs(&authorized);
    assert_eq!(
        next_step(&signing_status(&decode_v1_transaction(&done).unwrap(), &[])),
        NextSignature::Nothing
    );

    // It took a while. The blockhash the transaction was wrapped on is gone,
    // the authorization is as good as ever.
    let fresh = with_blockhash(&authorized, desk.chain.latest_blockhash()).unwrap();
    let landed = desk
        .chain
        .send_wire(&desk.hot_signs(&fresh))
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    println!(
        "wrapped transfer: {} bytes built, {} bytes wrapped, {} compute units",
        built.len(),
        landed.size,
        landed.compute_units
    );
    assert_eq!(desk.chain.balance(&recipient), SOL / 2);
    assert_eq!(desk.chain.balance(&desk.authority.pubkey()), 0);
}

/// Hot wallet → programmatic signer → Squads, with the wrapping done by the
/// panel: the built transaction is a plain propose, approve and execute in
/// which the member is the programmatic signer and the hot wallet pays rent.
#[test]
fn a_built_squads_proposal_by_a_programmatic_member_is_wrapped_and_lands() {
    for program in [SquadsProgram::V4, SquadsProgram::SmartAccount] {
        let mut desk = Desk::new();
        let recipient = KeypairSigner::new_random().pubkey();
        let setup = SquadsSetup {
            members: vec![SquadsMember {
                key: desk.address,
                permissions: PERMISSION_ALL,
            }],
            threshold: 1,
            time_lock: 0,
        };
        let squad = match program {
            SquadsProgram::V4 => {
                let create_key = KeypairSigner::new_random();
                let (squad, create) = Squad::create_v4(
                    &create_key.pubkey(),
                    &desk.hot.pubkey(),
                    &desk.chain.treasury,
                    &setup,
                );
                desk.chain
                    .send(&[&desk.hot, &create_key], vec![create])
                    .unwrap();
                squad
            }
            SquadsProgram::SmartAccount => {
                let config = desk.chain.data(&program.program_config());
                let count = SquadsProgram::smart_account_count(&config).unwrap();
                let (squad, create) = Squad::create_smart_account(
                    count + 1,
                    &desk.hot.pubkey(),
                    &desk.chain.treasury,
                    &setup,
                );
                desk.chain.send(&[&desk.hot], vec![create]).unwrap();
                squad
            }
        };
        let vault = squad.vault(0);
        desk.chain
            .send(&[&desk.hot], vec![pay(&desk.hot.pubkey(), &vault, 3 * SOL)])
            .unwrap();

        let payment = vec![pay(&vault, &recipient, SOL)];
        let built = desk.built(vec![
            squad
                .create_transaction(1, 0, &payment, &desk.address, &desk.hot.pubkey())
                .unwrap(),
            squad.create_proposal(1, &desk.address, &desk.hot.pubkey()),
            squad.approve(1, &desk.address),
            squad.execute(1, 0, &payment, &desk.address),
        ]);
        let wrapped = desk.wrap(&built);
        // The hot wallet is still the only key that signs the transaction,
        // now both as its payer and as the rent payer inside it.
        let tx = decode_v1_transaction(&wrapped).unwrap();
        assert_eq!(tx.signers.len(), 1, "{program:?}");

        let QrObject::Signature(signed) = desk.cold_signature(&wrapped) else {
            panic!("a signature object");
        };
        let (authorized, _) = accept_signature(&wrapped, &signed).unwrap();
        let landed = desk
            .chain
            .send_wire(&desk.hot_signs(&authorized))
            .unwrap_or_else(|failure| panic!("{program:?}: {}", logs(&failure)));
        println!(
            "{program:?} wrapped proposal: {} bytes built, {} bytes wrapped, {} compute units",
            built.len(),
            landed.size,
            landed.compute_units
        );
        assert_eq!(desk.chain.balance(&recipient), SOL, "{program:?}");
    }
}

/// The authorization took longer to come back than the blockhash lives. Sign
/// moves a relayed transaction to a fresh one before the payer signs, so it
/// still lands. Nothing does that for an ordinary transaction.
#[test]
fn sign_moves_a_relayed_transaction_off_an_expired_blockhash_and_leaves_a_plain_one() {
    use wallet_flows::sign::{sign, wants_fresh_blockhash};

    let mut desk = Desk::new();
    let recipient = KeypairSigner::new_random().pubkey();
    desk.chain.fund(&desk.address, SOL);
    let stale = desk.chain.latest_blockhash();

    let wrapped = desk.wrap(&desk.built(vec![pay(&desk.address, &recipient, SOL / 2)]));
    let QrObject::Signature(signed) = desk.cold_signature(&wrapped) else {
        panic!("a signature object");
    };
    let (authorized, _) = accept_signature(&wrapped, &signed).unwrap();
    let plain = desk.built(vec![pay(&desk.hot.pubkey(), &recipient, SOL / 10)]);

    // Meanwhile the chain moves on, and the blockhash both were built on is gone.
    let passer = KeypairSigner::new_random();
    desk.chain.fund(&passer.pubkey(), SOL);
    desk.chain
        .send(
            &[&passer],
            vec![pay(&passer.pubkey(), &recipient, SOL / 10)],
        )
        .unwrap();
    assert_ne!(desk.chain.latest_blockhash(), stale);
    // Signed as it is, the relayed transaction is refused for its blockhash.
    assert!(desk.chain.send_wire(&desk.hot_signs(&authorized)).is_err());
    assert_eq!(desk.chain.balance(&recipient), SOL / 10);

    let Desk { chain, hot, .. } = desk;
    let node = Node(Mutex::new(chain));
    let local = [hot.pubkey()];
    let keys: [&dyn WalletSigner; 1] = [&hot];

    let relayed = decode_v1_transaction(&authorized).unwrap();
    assert_eq!(relayed.lifetime, stale);
    assert!(wants_fresh_blockhash(&relayed, &local));
    let done = pollster::block_on(sign(Some(&node), &authorized, &keys)).unwrap();
    assert!(done.refreshed);
    assert_eq!(done.last_valid_height, Some(150));
    assert_eq!((done.authorizations, done.signatures), (0, 1));
    let moved = decode_v1_transaction(&done.wire).unwrap();
    assert_eq!(moved.lifetime, node.chain().latest_blockhash());
    assert!(authority_slots(&moved).iter().all(|slot| slot.signed));
    pollster::block_on(wallet_flows::send::submit(&node, &done.wire))
        .unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(node.chain().balance(&recipient), SOL / 10 + SOL / 2);

    // The ordinary transaction keeps the blockhash it came with, and the
    // chain refuses it.
    let ordinary = decode_v1_transaction(&plain).unwrap();
    assert!(!wants_fresh_blockhash(&ordinary, &local));
    let done = pollster::block_on(sign(Some(&node), &plain, &keys)).unwrap();
    assert!(!done.refreshed);
    assert_eq!(done.last_valid_height, None);
    assert_eq!(decode_v1_transaction(&done.wire).unwrap().lifetime, stale);
    let refused = pollster::block_on(wallet_flows::send::submit(&node, &done.wire)).unwrap_err();
    assert!(refused.to_string().contains("Blockhash"), "{refused}");
    assert_eq!(node.chain().balance(&recipient), SOL / 10 + SOL / 2);
}
