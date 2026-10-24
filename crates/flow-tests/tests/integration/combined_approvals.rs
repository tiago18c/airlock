//! Several members of one device finishing a proposal in one transaction,
//! on the path the app takes, against the real programs.
//!
//! The multisig's wallet names the wallets that approve together. The
//! Proposals modal reads the page, finds that they can cast every vote the
//! proposal still needs, and compiles approve, approve, execute. Sign rebuilds
//! it for the programmatic signers among them, each device signs its part,
//! and the exact bytes go to the chain.
//!
//! `programmatic_squads.rs` checks the raw shape of the same transaction,
//! built by hand.

use crate::support::node::Node;
use crate::support::{carried, Chain, SOL};
use solana_pubkey::Pubkey;
use std::future::Future;
use std::sync::Mutex;
use wallet_core::adapt::plan_adaptation;
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::proposals::{
    chosen_members, combined_approval, combined_instructions, CombinedApproval, ProposalEntry,
};
use wallet_core::protocol::QrObject;
use wallet_core::relay::{
    accept_signature, authority_signature_objects, authority_slots, SignatureUse,
};
use wallet_core::squads::{
    Squad, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL, PERMISSION_INITIATE,
};
use wallet_core::{
    decode_v1_transaction, parse_nonce_account, signature_objects, signing_status, DecodedTx,
    KeypairSigner, WalletId, WalletKind, WalletRecord, WalletSigner,
};
use wallet_flows::compile::{compile_fresh, compile_on_nonce};
use wallet_flows::sign::Signed;

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

fn run<T>(flow: impl Future<Output = T>) -> T {
    pollster::block_on(flow)
}

fn funded(chain: &mut Chain) -> KeypairSigner {
    let key = KeypairSigner::new_random();
    chain.fund(&key.pubkey(), 10 * SOL);
    key
}

fn hot_wallet(label: &str, key: &KeypairSigner) -> WalletRecord {
    WalletRecord::from_keypair_seed(label, key.seed_bytes())
}

/// A programmatic signer whose authority key is on another device.
fn signer_wallet(label: &str, authority: &KeypairSigner, nonce: Option<Pubkey>) -> WalletRecord {
    let mut wallet = WalletRecord::programmatic(label, authority.pubkey(), None);
    wallet.kind.set_executor_nonce(nonce);
    wallet
}

/// Create the executor nonce account of the programmatic signer `wallet`,
/// paid by `payer`, and note it on the wallet.
fn give_nonce(chain: &mut Chain, payer: &KeypairSigner, wallet: &mut WalletRecord) -> Pubkey {
    let (nonce, create) = create_executor_nonce(
        &payer.pubkey(),
        &payer.pubkey(),
        "airlock",
        &wallet.address().unwrap(),
        chain.nonce_rent(),
    )
    .unwrap();
    chain.send(&[payer], create.to_vec()).unwrap();
    wallet.kind.set_executor_nonce(Some(nonce));
    nonce
}

/// What the device holding `key` returns after it was shown `unsigned`: the
/// one authorization signature it made.
fn authorization_from(unsigned: &[u8], key: &KeypairSigner) -> QrObject {
    let signed = press_sign(None, unsigned, &[key]);
    assert_eq!((signed.authorizations, signed.signatures), (1, 0));
    let objects = authority_signature_objects(&decode_v1_transaction(&signed.wire).unwrap());
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].pubkey, key.pubkey());
    carried(&objects[0].signed)
}

/// Add a signature that came back from another device, as the Sign screen does.
fn add(wire: &[u8], object: QrObject) -> (Vec<u8>, SignatureUse) {
    let QrObject::Signature(signed) = object else {
        panic!("a signature object");
    };
    accept_signature(wire, &signed).unwrap()
}

/// Sign on a device that holds `keys`, pressing again for the transaction
/// when a press signed an authorization. `node` is there when the device is
/// online.
fn press_sign(node: Option<&Node>, wire: &[u8], keys: &[&KeypairSigner]) -> Signed {
    let signers: Vec<&dyn WalletSigner> =
        keys.iter().map(|key| *key as &dyn WalletSigner).collect();
    crate::support::sign_through(
        node.map(|node| node as &dyn wallet_core::SolanaRpc),
        wire,
        &signers,
        &mut wallet_flows::sign::VectorSigning::default(),
    )
    .unwrap_or_else(|err| panic!("{err}"))
}

fn signer_keys(tx: &DecodedTx) -> Vec<Pubkey> {
    tx.signers.iter().map(|slot| slot.pubkey).collect()
}

/// A multisig that needs two approvals, on the chain and as a wallet of this
/// device, with one proposal open that pays `recipient` one SOL.
struct Team {
    node: Node,
    squad: Squad,
    vault: Pubkey,
    recipient: Pubkey,
    /// The wallets of this device, as its vault holds them.
    wallets: Vec<WalletRecord>,
    multisig: WalletId,
}

impl Team {
    /// `voters` are the two members that may vote and execute. Somebody else
    /// stored the transaction and opened its proposal. The multisig's wallet
    /// is acted on by `acting`, and `together` approve with it.
    fn open(
        mut chain: Chain,
        program: SquadsProgram,
        voters: [Pubkey; 2],
        mut wallets: Vec<WalletRecord>,
        (acting, together): (WalletId, Vec<WalletId>),
        approval_nonce: Option<Pubkey>,
    ) -> Self {
        let creator = funded(&mut chain);
        let setup = SquadsSetup {
            members: voters
                .iter()
                .map(|key| (*key, PERMISSION_ALL))
                .chain([(creator.pubkey(), PERMISSION_INITIATE)])
                .map(|(key, permissions)| SquadsMember { key, permissions })
                .collect(),
            threshold: 2,
            time_lock: 0,
        };
        let squad = match program {
            SquadsProgram::V4 => {
                let create_key = KeypairSigner::new_random();
                let (squad, create) = Squad::create_v4(
                    &create_key.pubkey(),
                    &creator.pubkey(),
                    &chain.treasury,
                    &setup,
                );
                chain.send(&[&creator, &create_key], vec![create]).unwrap();
                squad
            }
            SquadsProgram::SmartAccount => {
                let config = chain.data(&program.program_config());
                let count = SquadsProgram::smart_account_count(&config).unwrap();
                let (squad, create) = Squad::create_smart_account(
                    count + 1,
                    &creator.pubkey(),
                    &chain.treasury,
                    &setup,
                );
                chain.send(&[&creator], vec![create]).unwrap();
                squad
            }
        };
        let vault = squad.vault(0);
        chain.fund(&vault, 3 * SOL);
        let recipient = KeypairSigner::new_random().pubkey();
        let payment = solana_system_interface::instruction::transfer(&vault, &recipient, SOL);
        let by = creator.pubkey();
        chain
            .send(
                &[&creator],
                vec![
                    squad
                        .create_transaction(1, 0, std::slice::from_ref(&payment), &by, &by)
                        .unwrap(),
                    squad.create_proposal(1, &by, &by),
                ],
            )
            .unwrap_or_else(|failure| panic!("{program:?}: {}", crate::support::logs(&failure)));

        // Added by address, as the Wallets screen does. The row then names
        // who acts and who approves with it.
        let mut multisig = WalletRecord::squads("Treasury", squad, Vec::new(), 2);
        multisig.kind.set_member_wallet(Some(acting));
        multisig.kind.set_approves_with(together);
        multisig.kind.set_approval_nonce(approval_nonce);
        let id = multisig.id;
        wallets.push(multisig);
        Self {
            node: Node(Mutex::new(chain)),
            squad,
            vault,
            recipient,
            wallets,
            multisig: id,
        }
    }

    /// What the Proposals modal finds for the open proposal: the page as the
    /// chain has it, the members the row names, and what they can do together.
    fn offer(&self) -> (ProposalEntry, CombinedApproval) {
        let (state, entries) = run(wallet_flows::squads::load_page(
            &self.node,
            &self.squad,
            None,
        ))
        .unwrap();
        let entry = entries.into_iter().find(|entry| entry.index == 1).unwrap();
        let chosen = chosen_members(&self.wallets, self.multisig, &state);
        let nonce = self
            .wallets
            .iter()
            .find(|wallet| wallet.id == self.multisig)
            .and_then(|wallet| match &wallet.kind {
                WalletKind::Squads { approval_nonce, .. } => *approval_nonce,
                _ => None,
            });
        let combined = combined_approval(&state, &entry, &chosen, nonce)
            .expect("the chosen members can finish the proposal together");
        (entry, combined)
    }

    /// Together, on a blockhash: compile what the modal offers with `payer`
    /// paying, then rebuild it on Sign for the programmatic signers in it.
    fn together(&self, payer: &Pubkey) -> Vec<u8> {
        let (entry, combined) = self.offer();
        assert_eq!(combined.durable_nonce, None);
        let instructions = combined_instructions(&self.squad, &entry, &combined).unwrap();
        let (built, _) = run(compile_fresh(&self.node, *payer, instructions)).unwrap();
        let tx = decode_v1_transaction(&built).unwrap();
        let plan = plan_adaptation(&tx, &self.wallets, None)
            .unwrap_or_else(|err| panic!("{err}"))
            .expect("a programmatic signer is asked to sign");
        let (adapted, _) = run(wallet_flows::adapt::rebuild(&self.node, &tx, &plan))
            .unwrap_or_else(|err| panic!("{err}"));
        adapted.wire
    }

    /// Blocks pass, and the blockhash of a moment ago is no longer accepted.
    fn wait(&self) {
        let mut chain = self.node.chain();
        let before = chain.latest_blockhash();
        let passer = funded(&mut chain);
        let treasury = chain.treasury;
        chain
            .send(
                &[&passer],
                vec![solana_system_interface::instruction::transfer(
                    &passer.pubkey(),
                    &treasury,
                    SOL,
                )],
            )
            .unwrap();
        assert_ne!(chain.latest_blockhash(), before);
    }

    fn submit(&self, wire: &[u8]) {
        run(wallet_flows::send::submit(&self.node, wire)).unwrap_or_else(|err| panic!("{err}"));
    }

    /// The proposal ran: the vault paid the recipient.
    fn assert_paid(&self) {
        let chain = self.node.chain();
        assert_eq!(chain.balance(&self.recipient), SOL);
        assert_eq!(chain.balance(&self.vault), 2 * SOL);
    }
}

#[test]
fn two_programmatic_members_on_two_devices_approve_and_execute_through_one_fee_payer() {
    for program in BOTH {
        let mut chain = Chain::new();
        let payer = funded(&mut chain);
        let (first, second) = (KeypairSigner::new_random(), KeypairSigner::new_random());
        let fees = hot_wallet("Fees", &payer);
        // The acting member has no nonce account. The other one's orders
        // the authorization both sign.
        let acting = signer_wallet("First", &first, None);
        let mut other = signer_wallet("Second", &second, None);
        let nonce = give_nonce(&mut chain, &payer, &mut other);
        let members = [acting.address().unwrap(), other.address().unwrap()];
        let choice = (acting.id, vec![other.id]);
        let team = Team::open(
            chain,
            program,
            members,
            vec![fees, acting, other],
            choice,
            None,
        );

        let (_, combined) = team.offer();
        assert_eq!(combined.voters, members.to_vec(), "{program:?}");
        assert_eq!(combined.executor, members[0], "{program:?}");

        let rebuilt = team.together(&payer.pubkey());
        let tx = decode_v1_transaction(&rebuilt).unwrap();
        assert_eq!(signer_keys(&tx), vec![payer.pubkey()], "{program:?}");
        assert_eq!(tx.instructions.len(), 1, "{program:?}");
        assert_eq!(authority_slots(&tx).len(), 2, "{program:?}");
        // The fee payer waits for both.
        let status = signing_status(&tx, &[payer.pubkey()]);
        assert_eq!(
            (status.signable, status.authorizations_elsewhere()),
            (0, 2),
            "{program:?}"
        );

        // Each device is shown the same unsigned transaction. The second
        // one's signature happens to come back first.
        let from_first = authorization_from(&rebuilt, &first);
        let from_second = authorization_from(&rebuilt, &second);
        let (wire, used) = add(&rebuilt, from_second);
        assert_eq!(used, SignatureUse::Authorization { dropped: 0 });
        let status = signing_status(&decode_v1_transaction(&wire).unwrap(), &[payer.pubkey()]);
        assert_eq!((status.signable, status.authorizations_elsewhere()), (0, 1));
        let (wire, _) = add(&wire, from_first);

        // That took longer than the blockhash lives.
        team.wait();
        let before = team.node.chain().nonce(&nonce);
        let done = press_sign(Some(&team.node), &wire, &[&payer]);
        assert!(done.refreshed && done.authorized, "{program:?}");
        assert_eq!((done.authorizations, done.signatures), (0, 1));
        team.submit(&done.wire);

        team.assert_paid();
        let chain = team.node.chain();
        assert_ne!(
            chain.nonce(&nonce),
            before,
            "{program:?}: the nonce was used"
        );
        // Neither authority and neither derived address holds a lamport.
        for unfunded in [first.pubkey(), second.pubkey(), members[0], members[1]] {
            assert_eq!(chain.balance(&unfunded), 0, "{program:?}");
        }
    }
}

#[test]
fn a_hot_key_member_and_a_programmatic_member_share_one_transaction() {
    for program in BOTH {
        for (member_pays, signer_first) in [(true, false), (false, false), (true, true)] {
            let what = format!(
                "{program:?}, the member pays: {member_pays}, the signer votes first: {signer_first}"
            );
            let mut chain = Chain::new();
            let member = funded(&mut chain);
            let fees = funded(&mut chain);
            let authority = KeypairSigner::new_random();
            let acting = hot_wallet("Member", &member);
            let mut other = signer_wallet("Signer", &authority, None);
            give_nonce(&mut chain, &fees, &mut other);
            let members = [acting.address().unwrap(), other.address().unwrap()];
            // With the signer first it also executes, after the hot key's
            // vote: both of its instructions sit in one `Submit`.
            let choice = if signer_first {
                (other.id, vec![acting.id])
            } else {
                (acting.id, vec![other.id])
            };
            let wallets = vec![hot_wallet("Fees", &fees), acting, other];
            let team = Team::open(chain, program, members, wallets, choice, None);
            let payer = if member_pays { &member } else { &fees };

            let rebuilt = team.together(&payer.pubkey());
            let tx = decode_v1_transaction(&rebuilt).unwrap();
            // The hot key still signs the transaction, once.
            let expected = if member_pays {
                vec![member.pubkey()]
            } else {
                vec![fees.pubkey(), member.pubkey()]
            };
            assert_eq!(signer_keys(&tx), expected, "{what}");
            assert_eq!(authority_slots(&tx).len(), 1, "{what}");

            let (wire, _) = add(&rebuilt, authorization_from(&rebuilt, &authority));
            team.wait();
            // This device holds every key that signs the transaction.
            let done = press_sign(Some(&team.node), &wire, &[&fees, &member]);
            assert!(done.refreshed, "{what}");
            assert_eq!(done.signatures, expected.len(), "{what}");
            team.submit(&done.wire);
            team.assert_paid();
        }
    }
}

#[test]
fn an_offline_member_and_a_hot_key_approve_and_execute_on_the_rows_durable_nonce() {
    for program in BOTH {
        for offline_controls_nonce in [false, true] {
            let what = format!(
                "{program:?}, the offline key controls the nonce: {offline_controls_nonce}"
            );
            let mut chain = Chain::new();
            let hot = funded(&mut chain);
            let cold = KeypairSigner::new_random();
            let nonce_key = KeypairSigner::new_random();
            let nonce = nonce_key.pubkey();
            let authority = if offline_controls_nonce {
                cold.pubkey()
            } else {
                hot.pubkey()
            };
            let create = solana_system_interface::instruction::create_nonce_account(
                &hot.pubkey(),
                &nonce,
                &authority,
                chain.rent(80),
            );
            chain.send(&[&hot, &nonce_key], create).unwrap();
            let hash_of = |team: &Team| {
                let account = team.node.chain().account(&nonce).unwrap();
                parse_nonce_account(nonce, &account).unwrap().hash
            };

            let acting = hot_wallet("Hot", &hot);
            let other = WalletRecord::offline("Cold", cold.pubkey());
            let members = [hot.pubkey(), cold.pubkey()];
            let choice = (acting.id, vec![other.id]);
            let team = Team::open(
                chain,
                program,
                members,
                vec![acting, other],
                choice,
                Some(nonce),
            );

            let (entry, combined) = team.offer();
            assert_eq!(combined.voters, members.to_vec(), "{what}");
            assert_eq!(combined.durable_nonce, Some(nonce), "{what}");
            let instructions = combined_instructions(&team.squad, &entry, &combined).unwrap();
            let signers: Vec<Pubkey> = std::iter::once(hot.pubkey())
                .chain(combined.signers())
                .collect();
            let unsigned = run(compile_on_nonce(
                &team.node,
                hot.pubkey(),
                instructions,
                nonce,
                &signers,
            ))
            .unwrap_or_else(|err| panic!("{what}: {err}"));
            let tx = decode_v1_transaction(&unsigned).unwrap();
            assert!(tx.uses_durable_nonce, "{what}");
            assert_eq!(signer_keys(&tx), members.to_vec(), "{what}");
            // No keyless wallet is in it, so nothing is rebuilt.
            assert_eq!(plan_adaptation(&tx, &team.wallets, None), Ok(None));
            let before = hash_of(&team);
            assert_eq!(tx.lifetime, before, "{what}");

            // The payer signs first. Nothing can go stale.
            let paid = press_sign(Some(&team.node), &unsigned, &[&hot]);
            assert_eq!((paid.signatures, paid.refreshed), (1, false), "{what}");

            // The offline wallet signs the same transaction on its own
            // device, much later, and returns one signature.
            for _ in 0..3 {
                team.wait();
            }
            let cold_signed = press_sign(None, &unsigned, &[&cold]);
            let objects = signature_objects(&decode_v1_transaction(&cold_signed.wire).unwrap());
            assert_eq!(objects.len(), 1, "{what}");
            let (wire, used) = add(&paid.wire, carried(&objects[0].signed));
            assert_eq!(used, SignatureUse::Transaction, "{what}");
            assert!(decode_v1_transaction(&wire).unwrap().fully_signed());

            team.submit(&wire);
            team.assert_paid();
            assert_ne!(hash_of(&team), before, "{what}: the nonce advanced");
        }
    }
}
