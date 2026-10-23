//! Hot wallet → programmatic signer → Squads, against the real programs.
//!
//! The roles:
//!
//! - The **hot wallet** pays every fee and signs every transaction that
//!   reaches the chain. It is not a member of the multisig.
//! - The **authority** is an Ed25519 key that never signs a transaction and
//!   never holds a lamport. It only signs authorization messages. Those carry
//!   no blockhash, so it can sign them at any time, offline.
//! - The authority's **programmatic signer address** is the multisig member.

use crate::support::{logs, Chain, Landed, SMART_ACCOUNTS_ALREADY_CREATED, SOL};
use std::collections::BTreeMap;
use wallet_core::programmatic::{
    create_executor_nonce, programmatic_signer_address, Authorization, Execution, ExecutorNonce,
    Submission, EXECUTOR_NONCE_RENT_LAMPORTS,
};
use wallet_core::squads::{
    Squad, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL, PERMISSION_VOTE,
};
use wallet_core::{ComputeBudgetSpec, Instruction, KeypairSigner, WalletSigner};

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

/// Everything a flow starts from: a funded hot wallet, an unfunded authority,
/// a nonce the authority's derived address controls, and a multisig.
struct World {
    chain: Chain,
    hot: KeypairSigner,
    authority: KeypairSigner,
    /// The authority's programmatic signer address.
    member: solana_pubkey::Pubkey,
    nonce: solana_pubkey::Pubkey,
    squad: Squad,
    recipient: solana_pubkey::Pubkey,
}

impl World {
    /// `others` are further members that sign as ordinary keys.
    fn new(program: SquadsProgram, threshold: u16, others: &[&KeypairSigner]) -> Self {
        let mut chain = Chain::new();
        let hot = KeypairSigner::new_random();
        let authority = KeypairSigner::new_random();
        let member = programmatic_signer_address(&authority.pubkey());
        chain.fund(&hot.pubkey(), 10 * SOL);

        // The nonce sits at an address derived from the hot wallet, so the hot
        // wallet's one signature creates it.
        let (nonce, create_nonce) = create_executor_nonce(
            &hot.pubkey(),
            &hot.pubkey(),
            "airlock",
            &member,
            chain.nonce_rent(),
        )
        .unwrap();
        chain.send(&[&hot], create_nonce.to_vec()).unwrap();
        assert_eq!(chain.nonce(&nonce).authority, member);
        // The app has no way to ask for the rate, so its constant must cover it.
        assert!(chain.nonce_rent() <= EXECUTOR_NONCE_RENT_LAMPORTS);

        let mut members = vec![SquadsMember {
            key: member,
            permissions: PERMISSION_ALL,
        }];
        members.extend(others.iter().map(|other| SquadsMember {
            key: other.pubkey(),
            permissions: PERMISSION_VOTE,
        }));
        let setup = SquadsSetup {
            members,
            threshold,
            time_lock: 0,
        };
        let squad = create_squad(&mut chain, program, &hot, &setup);

        // Fund the vault, which is where a multisig's money lives.
        let vault = squad.vault(0);
        let fund = solana_system_interface::instruction::transfer(&hot.pubkey(), &vault, 3 * SOL);
        chain.send(&[&hot], vec![fund]).unwrap();

        Self {
            chain,
            hot,
            authority,
            member,
            nonce,
            squad,
            recipient: KeypairSigner::new_random().pubkey(),
        }
    }

    /// What the vault is asked to do: pay the recipient one SOL.
    fn payment(&self) -> Vec<Instruction> {
        vec![solana_system_interface::instruction::transfer(
            &self.squad.vault(0),
            &self.recipient,
            SOL,
        )]
    }

    fn propose(&self) -> Vec<Instruction> {
        vec![
            self.squad
                .create_transaction(1, 0, &self.payment(), &self.member, &self.hot.pubkey())
                .unwrap(),
            self.squad
                .create_proposal(1, &self.member, &self.hot.pubkey()),
        ]
    }

    fn approve(&self) -> Vec<Instruction> {
        vec![self.squad.approve(1, &self.member)]
    }

    fn execute(&self) -> Vec<Instruction> {
        vec![self.squad.execute(1, 0, &self.payment(), &self.member)]
    }

    /// The authority's part. Nothing here touches the chain.
    fn authorize(&self, instructions: &[Instruction], nonce: ExecutorNonce) -> Signed {
        let authorization =
            Authorization::new(self.member, instructions, &[self.authority.pubkey()], nonce)
                .unwrap();
        let signature = authorization.sign(&self.authority).unwrap();
        Signed {
            submit: authorization
                .submit_instruction(&BTreeMap::from([(self.authority.pubkey(), signature)]))
                .unwrap(),
            signed_bytes: authorization.signing_bytes().len(),
            next: authorization.next_nonce(),
        }
    }

    /// The hot wallet's part: wrap the signed authorization in a transaction,
    /// pay for it, send it.
    fn relay(&mut self, signed: &Signed) -> Result<Landed, String> {
        self.chain
            .send_with(&[&self.hot], vec![signed.submit.clone()], relay_budget())
            .map_err(|failure| logs(&failure))
    }
}

/// Room for a `Submit` that runs a proposal's steps inside it.
fn relay_budget() -> ComputeBudgetSpec {
    ComputeBudgetSpec {
        cu_limit: Some(400_000),
        ..ComputeBudgetSpec::default()
    }
}

/// Create the multisig and check the chain stored the members that were asked for.
fn create_squad(
    chain: &mut Chain,
    program: SquadsProgram,
    hot: &KeypairSigner,
    setup: &SquadsSetup,
) -> Squad {
    let squad = match program {
        SquadsProgram::V4 => {
            let create_key = KeypairSigner::new_random();
            let (squad, create) =
                Squad::create_v4(&create_key.pubkey(), &hot.pubkey(), &chain.treasury, setup);
            chain.send(&[hot, &create_key], vec![create]).unwrap();
            squad
        }
        SquadsProgram::SmartAccount => {
            let config = chain.data(&program.program_config());
            let count = SquadsProgram::smart_account_count(&config).unwrap();
            assert_eq!(count, SMART_ACCOUNTS_ALREADY_CREATED);
            let (squad, create) =
                Squad::create_smart_account(count + 1, &hot.pubkey(), &chain.treasury, setup);
            chain.send(&[hot], vec![create]).unwrap();
            squad
        }
    };
    // What the program stored is what was asked for, read back the way the
    // app reads it. Squads keeps members sorted by address.
    let state = squad
        .parse_state(&chain.data(&squad.address))
        .unwrap_or_else(|| panic!("{program:?} was not created"));
    let mut expected = setup.members.clone();
    expected.sort_by_key(|member| member.key);
    assert_eq!(state.members, expected, "{program:?}");
    assert_eq!(
        (state.threshold, state.transaction_index),
        (setup.threshold, 0)
    );
    squad
}

/// Every `authorities` entry signs the same authorization. `payer` is the
/// execution message's first account. Nothing is charged to it.
fn authorize_with(
    payer: solana_pubkey::Pubkey,
    instructions: &[Instruction],
    authorities: &[&KeypairSigner],
    nonce: ExecutorNonce,
) -> Signed {
    let keys: Vec<_> = authorities
        .iter()
        .map(|authority| authority.pubkey())
        .collect();
    let authorization = Authorization::new(payer, instructions, &keys, nonce).unwrap();
    let mut signatures = BTreeMap::new();
    for authority in authorities {
        signatures.insert(authority.pubkey(), authorization.sign(*authority).unwrap());
    }
    Signed {
        submit: authorization.submit_instruction(&signatures).unwrap(),
        signed_bytes: authorization.signing_bytes().len(),
        next: authorization.next_nonce(),
    }
}

struct Signed {
    submit: Instruction,
    /// Size of what the authority signed. This is what crosses a QR code.
    signed_bytes: usize,
    next: ExecutorNonce,
}

fn report(program: SquadsProgram, step: &str, signed: &Signed, landed: Landed) {
    println!(
        "{program:?} {step}: authority signs {} bytes, relay transaction {} bytes, {} compute units",
        signed.signed_bytes, landed.size, landed.compute_units
    );
}

/// The whole life of a payment. The authority signs all three steps before any
/// of them is sent, each against the nonce value the one before it will leave.
#[test]
fn an_offline_authority_proposes_approves_and_executes_through_a_hot_wallet() {
    for program in BOTH {
        let mut world = World::new(program, 1, &[]);
        let start = world.chain.nonce(&world.nonce);

        let propose = world.authorize(&world.propose(), start);
        let approve = world.authorize(&world.approve(), propose.next);
        let execute = world.authorize(&world.execute(), approve.next);

        for (step, signed) in [
            ("propose", &propose),
            ("approve", &approve),
            ("execute", &execute),
        ] {
            let landed = world
                .relay(signed)
                .unwrap_or_else(|err| panic!("{program:?} {step}: {err}"));
            report(program, step, signed, landed);
            assert_eq!(
                world.chain.nonce(&world.nonce),
                signed.next,
                "{program:?} {step}"
            );
        }

        assert_eq!(world.chain.balance(&world.recipient), SOL, "{program:?}");
        assert_eq!(world.chain.balance(&world.squad.vault(0)), 2 * SOL);
        let state = world
            .squad
            .parse_state(&world.chain.data(&world.squad.address))
            .unwrap();
        assert_eq!(state.transaction_index, 1);
        // The authority never needed funds, and its derived address holds none.
        assert_eq!(world.chain.balance(&world.authority.pubkey()), 0);
        assert_eq!(world.chain.balance(&world.member), 0);
    }
}

/// With a threshold of one, a single signature and a single transaction do
/// what otherwise takes three.
#[test]
fn one_signature_can_carry_the_whole_proposal() {
    for program in BOTH {
        let mut world = World::new(program, 1, &[]);
        let everything: Vec<Instruction> = [world.propose(), world.approve(), world.execute()]
            .into_iter()
            .flatten()
            .collect();
        let signed = world.authorize(&everything, world.chain.nonce(&world.nonce));
        let landed = world
            .relay(&signed)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        report(program, "all at once", &signed, landed);
        assert_eq!(world.chain.balance(&world.recipient), SOL, "{program:?}");
    }
}

/// An authorization runs once, and only in the order it was signed for.
#[test]
fn a_relay_cannot_replay_or_reorder_what_was_signed() {
    for program in BOTH {
        let mut world = World::new(program, 1, &[]);
        let start = world.chain.nonce(&world.nonce);
        let propose = world.authorize(&world.propose(), start);
        let approve = world.authorize(&world.approve(), propose.next);

        // Approve was signed to follow propose. On its own it is refused, and
        // refusing it leaves the nonce where it was.
        let early = world.relay(&approve).unwrap_err();
        assert!(early.contains("Custom"), "{program:?}: {early}");
        assert_eq!(world.chain.nonce(&world.nonce), start);

        world.relay(&propose).unwrap();
        let again = world.relay(&propose).unwrap_err();
        assert!(again.contains("Custom"), "{program:?}: {again}");

        world.relay(&approve).unwrap();
        let again = world.relay(&approve).unwrap_err();
        assert!(again.contains("Custom"), "{program:?}: {again}");
        assert_eq!(world.chain.nonce(&world.nonce), approve.next);
    }
}

/// A relay holds a valid signature and still cannot change what it covers.
#[test]
fn a_relay_cannot_alter_what_was_signed() {
    for program in BOTH {
        let mut world = World::new(program, 1, &[]);
        let propose = world.authorize(&world.propose(), world.chain.nonce(&world.nonce));
        world.relay(&propose).unwrap();

        let mut approve = world.authorize(&world.approve(), propose.next);
        // Flip the last byte of the authorization message: the approve memo flag.
        *approve.submit.data.last_mut().unwrap() ^= 1;
        let refused = world.relay(&approve).unwrap_err();
        // The signer program's `InvalidSignature`.
        assert!(refused.contains("Custom(3)"), "{program:?}: {refused}");
    }
}

/// A multisig can mix both kinds of member. Here the threshold is two: the
/// programmatic signer and an ordinary key that signs its own transaction.
#[test]
fn a_programmatic_member_and_an_ordinary_member_reach_the_threshold_together() {
    for program in BOTH {
        let other = KeypairSigner::new_random();
        let mut world = World::new(program, 2, &[&other]);
        world.chain.fund(&other.pubkey(), SOL);
        let start = world.chain.nonce(&world.nonce);

        let propose = world.authorize(&world.propose(), start);
        let approve = world.authorize(&world.approve(), propose.next);
        let execute = world.authorize(&world.execute(), approve.next);
        world.relay(&propose).unwrap();
        world.relay(&approve).unwrap();

        // One approval of two: not yet.
        let early = world.relay(&execute).unwrap_err();
        assert!(early.contains("Custom"), "{program:?}: {early}");
        assert_eq!(world.chain.balance(&world.recipient), 0);

        // The ordinary member approves directly, paying its own fee.
        let vote = world.squad.approve(1, &other.pubkey());
        world
            .chain
            .send(&[&other], vec![vote])
            .unwrap_or_else(|failure| panic!("{program:?}: {}", logs(&failure)));

        // The refused attempt did not consume the nonce, so the same signed
        // execute is still good.
        world
            .relay(&execute)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        assert_eq!(world.chain.balance(&world.recipient), SOL, "{program:?}");
    }
}

/// A 2-of-2 whose members are both programmatic signers. A third key pays the
/// fee and is not a member. The proposal is opened first. Both authorities
/// then sign one authorization, and one relay runs it:
///
/// ```text
/// outer transaction       the third key signs and pays
/// └─ Submit               both message signatures
///    └─ Execute
///       ├─ approve 1
///       ├─ approve 2
///       └─ execute
/// ```
#[test]
fn a_fee_payer_relays_both_programmatic_members_approving_and_executing() {
    for program in BOTH {
        let mut chain = Chain::new();
        let hot = KeypairSigner::new_random();
        let first = KeypairSigner::new_random();
        let second = KeypairSigner::new_random();
        let members = [
            programmatic_signer_address(&first.pubkey()),
            programmatic_signer_address(&second.pubkey()),
        ];
        chain.fund(&hot.pubkey(), 10 * SOL);

        // The nonce is controlled by the first member. The second signs only
        // the instructions that name its address.
        let (nonce, create_nonce) = create_executor_nonce(
            &hot.pubkey(),
            &hot.pubkey(),
            "airlock",
            &members[0],
            chain.nonce_rent(),
        )
        .unwrap();
        chain.send(&[&hot], create_nonce.to_vec()).unwrap();
        assert_eq!(chain.nonce(&nonce).authority, members[0]);

        let squad = create_squad(
            &mut chain,
            program,
            &hot,
            &SquadsSetup {
                members: vec![
                    SquadsMember {
                        key: members[0],
                        permissions: PERMISSION_ALL,
                    },
                    SquadsMember {
                        key: members[1],
                        permissions: PERMISSION_ALL,
                    },
                ],
                threshold: 2,
                time_lock: 0,
            },
        );
        let vault = squad.vault(0);
        let recipient = KeypairSigner::new_random().pubkey();
        let fund = solana_system_interface::instruction::transfer(&hot.pubkey(), &vault, 3 * SOL);
        chain.send(&[&hot], vec![fund]).unwrap();

        let payment = vec![solana_system_interface::instruction::transfer(
            &vault, &recipient, SOL,
        )];
        let propose = vec![
            squad
                .create_transaction(1, 0, &payment, &members[0], &hot.pubkey())
                .unwrap(),
            squad.create_proposal(1, &members[0], &hot.pubkey()),
        ];
        let steps = vec![
            squad.approve(1, &members[0]),
            squad.approve(1, &members[1]),
            squad.execute(1, 0, &payment, &members[0]),
        ];

        // Both authorities sign the execute authorization before either relay
        // is sent. It is bound to the nonce the proposal will leave behind.
        let start = chain.nonce(&nonce);
        let proposed = authorize_with(members[0], &propose, &[&first], start);
        let executed = authorize_with(members[0], &steps, &[&first, &second], proposed.next);

        let opened = chain
            .send_with(&[&hot], vec![proposed.submit.clone()], relay_budget())
            .unwrap_or_else(|failure| panic!("{program:?} propose: {}", logs(&failure)));
        report(program, "propose", &proposed, opened);
        assert_eq!(chain.balance(&recipient), 0);
        assert_eq!(chain.nonce(&nonce), proposed.next);

        // The outer transaction has one signer, the fee payer, and one
        // instruction, the signer program. Both approvals are inside it.
        let budget = relay_budget();
        let compiled = wallet_core::compile_v1_budget(
            hot.pubkey(),
            vec![executed.submit.clone()],
            None,
            Some(chain.latest_blockhash()),
            budget.clone(),
        )
        .unwrap();
        let outer = wallet_core::decode_v1_transaction(&compiled.wire).unwrap();
        assert_eq!(outer.signers.len(), 1, "{program:?}");
        assert_eq!(outer.fee_payer(), Some(hot.pubkey()), "{program:?}");
        assert_eq!(outer.instructions.len(), 1, "{program:?}");
        assert_eq!(outer.instructions[0].data, executed.submit.data);
        assert!(
            executed.submit.accounts.iter().all(|meta| !meta.is_signer),
            "{program:?}: the fee payer is not an inner signer"
        );

        let submission = Submission::parse(&executed.submit.data).unwrap();
        let mut signed_by: Vec<_> = submission.authorities().collect();
        signed_by.sort();
        let mut expected = vec![(first.pubkey(), true), (second.pubkey(), true)];
        expected.sort();
        assert_eq!(signed_by, expected, "{program:?}");

        let ran = Execution::parse(&submission.execute().unwrap())
            .unwrap()
            .instructions();
        assert_eq!(ran.len(), steps.len(), "{program:?}");
        for (ran, step) in ran.iter().zip(&steps) {
            assert_eq!(ran.program_id, step.program_id, "{program:?}");
            assert_eq!(ran.data, step.data, "{program:?}");
            let ran_keys: Vec<_> = ran.accounts.iter().map(|meta| meta.pubkey).collect();
            let step_keys: Vec<_> = step.accounts.iter().map(|meta| meta.pubkey).collect();
            assert_eq!(ran_keys, step_keys, "{program:?}");
        }
        assert!(
            ran[0]
                .accounts
                .iter()
                .any(|meta| meta.pubkey == members[0] && meta.is_signer),
            "{program:?}"
        );
        assert!(
            ran[1]
                .accounts
                .iter()
                .any(|meta| meta.pubkey == members[1] && meta.is_signer),
            "{program:?}"
        );

        let landed = chain
            .send_with(&[&hot], vec![executed.submit.clone()], budget)
            .unwrap_or_else(|failure| panic!("{program:?} execute: {}", logs(&failure)));
        report(program, "approve, approve, execute", &executed, landed);
        assert_eq!(chain.nonce(&nonce), executed.next, "{program:?}");
        assert_eq!(chain.balance(&recipient), SOL, "{program:?}");
        assert_eq!(chain.balance(&vault), 2 * SOL, "{program:?}");
        // Neither authority pays, and neither derived address holds lamports.
        assert_eq!(chain.balance(&first.pubkey()), 0);
        assert_eq!(chain.balance(&second.pubkey()), 0);
        assert_eq!(chain.balance(&members[0]), 0);
        assert_eq!(chain.balance(&members[1]), 0);
    }
}

/// The authority's own key is not a member. Signing a transaction with it
/// directly gets nowhere, which is why the member is the derived address.
#[test]
fn the_authority_key_itself_is_not_a_member() {
    for program in BOTH {
        let mut world = World::new(program, 1, &[]);
        let propose = world.authorize(&world.propose(), world.chain.nonce(&world.nonce));
        world.relay(&propose).unwrap();

        let authority = KeypairSigner::from_seed(&world.authority.seed_bytes()).unwrap();
        let vote = world.squad.approve(1, &authority.pubkey());
        let refused = world
            .chain
            .send(&[&world.hot, &authority], vec![vote])
            .unwrap_err();
        assert!(logs(&refused).contains("Custom"), "{program:?}");
    }
}

/// How much fits. A payment is stored inside the proposal, which is inside the
/// execution message, which is inside the authorization message, which is the
/// data of one instruction of a 4096-byte transaction.
#[test]
fn a_proposal_paying_many_recipients_fits_until_the_transaction_is_full() {
    for program in BOTH {
        let mut most = 0;
        for recipients in [1usize, 5, 10, 20, 30, 40, 50] {
            let mut world = World::new(program, 1, &[]);
            let vault = world.squad.vault(0);
            let payment: Vec<Instruction> = (0..recipients)
                .map(|_| {
                    solana_system_interface::instruction::transfer(
                        &vault,
                        &KeypairSigner::new_random().pubkey(),
                        SOL / 100,
                    )
                })
                .collect();
            let propose = vec![
                world
                    .squad
                    .create_transaction(1, 0, &payment, &world.member, &world.hot.pubkey())
                    .unwrap(),
                world
                    .squad
                    .create_proposal(1, &world.member, &world.hot.pubkey()),
            ];
            let nonce = world.chain.nonce(&world.nonce);
            let Ok(authorization) =
                Authorization::new(world.member, &propose, &[world.authority.pubkey()], nonce)
            else {
                break;
            };
            let signature = authorization.sign(&world.authority).unwrap();
            let submit = authorization
                .submit_instruction(&BTreeMap::from([(world.authority.pubkey(), signature)]))
                .unwrap();
            let size = authorization.signing_bytes().len();
            if size + 400 > wallet_core::MAX_TRANSACTION_SIZE {
                break;
            }
            let signed = Signed {
                submit,
                signed_bytes: size,
                next: authorization.next_nonce(),
            };
            match world.relay(&signed) {
                Ok(landed) => {
                    report(
                        program,
                        &format!("propose {recipients} payments"),
                        &signed,
                        landed,
                    );
                    most = recipients;
                }
                Err(err) => {
                    println!("{program:?} propose {recipients} payments: refused\n{err}");
                    break;
                }
            }
        }
        assert!(most >= 10, "{program:?} fitted only {most}");
    }
}
