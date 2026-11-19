//! Executing a proposal that reached its threshold, on the path a person
//! takes, against the real programs: the Proposals modal reads the page and
//! offers Execute, the transaction is compiled as it does, Review and Sign
//! simulates and fits it, the keys sign, it is submitted, and the modal,
//! opened again, reads the proposal as executed.
//!
//! Each case is one the app must either carry out or refuse in a plain
//! sentence before anything is signed: approvals made elsewhere, a time
//! lock, a multisig that changed since, members that may only vote or only
//! execute, members without a key or on another device, heavy transactions,
//! ephemeral signers, and transactions that read accounts from lookup
//! tables.

use crate::support::node::Node;
use crate::support::vector::Desk as VectorDesk;
use crate::support::{carried, logs, Chain, SOL};
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Mutex;
use wallet_core::adapt::plan_adaptation;
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::proposals::{
    available_action, chosen_members, executor_instead, MemberKind, NoAction, ProposalAction,
    ProposalEntry, ProposalRequest, Stored,
};
use wallet_core::protocol::QrObject;
use wallet_core::relay::{accept_signature, authority_signature_objects};
use wallet_core::squads::{
    ProposalStatus, Squad, SquadState, SquadsMember, SquadsProgram, SquadsSetup, TableProblem,
    PERMISSION_ALL, PERMISSION_EXECUTE, PERMISSION_INITIATE, PERMISSION_VOTE,
};
use wallet_core::{
    decode_v1_transaction, stored_in, BuildError, Instruction, KeypairSigner, StoredTarget,
    WalletRecord, WalletSigner,
};
use wallet_flows::compile::compile_fresh;
use wallet_flows::sign::Signed;

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

/// Payments in the stored transaction that needs more compute than one
/// instruction gets by default. Many more do not fit the heap the programs
/// store a transaction with.
const PAYMENTS: u64 = 60;

fn run<T>(flow: impl std::future::Future<Output = T>) -> T {
    pollster::block_on(flow)
}

fn funded(chain: &mut Chain) -> KeypairSigner {
    let key = KeypairSigner::new_random();
    chain.fund(&key.pubkey(), 10 * SOL);
    key
}

/// Sign on a device that holds `keys`, as the Sign screen does: one press,
/// and a second when the first signed an authorization.
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

/// Create a multisig of `members`, paid by `creator`, with three SOL in
/// its vault.
fn create(
    chain: &mut Chain,
    program: SquadsProgram,
    creator: &KeypairSigner,
    members: &[(Pubkey, u8)],
    threshold: u16,
    time_lock: u32,
    config_authority: Option<Pubkey>,
) -> Squad {
    let setup = SquadsSetup {
        members: members
            .iter()
            .map(|(key, permissions)| SquadsMember {
                key: *key,
                permissions: *permissions,
            })
            .collect(),
        threshold,
        time_lock,
        config_authority,
        rent_collector: None,
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
            chain.send(&[creator, &create_key], vec![create]).unwrap();
            squad
        }
        SquadsProgram::SmartAccount => {
            let config = chain.data(&program.program_config());
            let count = SquadsProgram::smart_account_count(&config).unwrap();
            let (squad, create) =
                Squad::create_smart_account(count + 1, &creator.pubkey(), &chain.treasury, &setup);
            chain.send(&[creator], vec![create]).unwrap();
            squad
        }
    };
    chain.fund(&squad.vault(0), 3 * SOL);
    squad
}

/// A multisig on the chain, a proposer that may only propose, members that
/// vote, and a hot wallet that pays fees and is no member.
struct Team {
    node: Node,
    squad: Squad,
    vault: Pubkey,
    proposer: KeypairSigner,
    fees: KeypairSigner,
    recipient: Pubkey,
}

impl Team {
    /// `members` besides the proposer, on `chain`.
    fn on(
        mut chain: Chain,
        program: SquadsProgram,
        members: &[(Pubkey, u8)],
        threshold: u16,
        time_lock: u32,
        config_authority: Option<Pubkey>,
    ) -> Self {
        let fees = funded(&mut chain);
        let proposer = funded(&mut chain);
        let all: Vec<(Pubkey, u8)> = members
            .iter()
            .copied()
            .chain([(proposer.pubkey(), PERMISSION_INITIATE)])
            .collect();
        let squad = create(
            &mut chain,
            program,
            &fees,
            &all,
            threshold,
            time_lock,
            config_authority,
        );
        Self {
            node: Node(Mutex::new(chain)),
            vault: squad.vault(0),
            squad,
            proposer,
            fees,
            recipient: KeypairSigner::new_random().pubkey(),
        }
    }

    fn payment(&self, lamports: u64) -> Instruction {
        solana_system_interface::instruction::transfer(&self.vault, &self.recipient, lamports)
    }

    /// The proposer stores `instructions` as transaction `index` and opens
    /// voting on it.
    fn propose(&self, index: u64, instructions: &[Instruction]) {
        let by = self.proposer.pubkey();
        let store = self
            .squad
            .create_transaction(index, 0, instructions, &by, &by)
            .unwrap();
        self.propose_with(index, store);
    }

    /// [`Self::propose`] with the instruction that stores it already made.
    fn propose_with(&self, index: u64, store: Instruction) {
        let by = self.proposer.pubkey();
        let open = self.squad.create_proposal(index, &by, &by);
        let sent = self.node.chain().send(&[&self.proposer], vec![store, open]);
        sent.unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    }

    /// `member` approves transaction `index` in a transaction of its own.
    fn approve(&self, index: u64, member: &KeypairSigner) {
        let approve = self.squad.approve(index, &member.pubkey());
        let sent = self.node.chain().send(&[&self.fees, member], vec![approve]);
        sent.unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    }

    /// What the modal reads when it opens: the multisig and entry `index`.
    fn modal(&self, index: u64) -> (SquadState, ProposalEntry) {
        let (state, entries) = run(wallet_flows::squads::load_page(
            &self.node,
            &self.squad,
            None,
        ))
        .unwrap_or_else(|err| panic!("{err}"));
        let entry = entries
            .into_iter()
            .find(|entry| entry.index == index)
            .unwrap();
        (state, entry)
    }

    /// What pressing the row's button does: the instructions of `request`,
    /// compiled on a fresh blockhash with `payer` paying.
    fn build(&self, index: u64, request: &ProposalRequest, payer: &Pubkey) -> Vec<u8> {
        let (_, entry) = self.modal(index);
        let (wire, _) = run(wallet_flows::squads::compile_request(
            &self.node,
            &self.squad,
            &entry,
            request,
            *payer,
            |_| None,
            None,
        ))
        .unwrap_or_else(|err| panic!("{err}"));
        wire
    }

    /// Review and Sign: simulated at the highest limit, then fitted to what
    /// it used. (The screen also asks for balances after, which the test
    /// chain does not serve; the limits are fitted the same way.)
    fn review(&self, wire: &[u8]) -> Vec<u8> {
        let probe = wallet_core::relay::with_compute_unit_limit(wire, u32::MAX).unwrap();
        let simulated = self.node.chain().simulate_wire(&probe);
        if let Err(failure) = simulated {
            panic!("the simulation failed: {}", logs(&failure));
        }
        run(wallet_flows::adapt::fit_limits_from_measure(
            &self.node, wire,
        ))
        .unwrap()
    }

    fn submit(&self, wire: &[u8]) {
        run(wallet_flows::send::submit(&self.node, wire)).unwrap_or_else(|err| panic!("{err}"));
    }

    /// Execute `index` as `member`, alone, all the way, with `keys` signing.
    fn execute_as(&self, index: u64, member: &Pubkey, keys: &[&KeypairSigner]) {
        let (state, entry) = self.modal(index);
        let now = self.node.chain().now();
        assert_eq!(
            available_action(&state, &entry, member, now),
            Ok(ProposalAction::Execute)
        );
        let request = ProposalRequest::Single {
            member: *member,
            action: ProposalAction::Execute,
        };
        let wire = self.review(&self.build(index, &request, &self.fees.pubkey()));
        let signed = press_sign(Some(&self.node), &wire, keys);
        self.submit(&signed.wire);
    }

    /// The modal, opened again, reads `index` as executed, with nothing left
    /// to do for `member`.
    fn assert_executed(&self, index: u64, member: &Pubkey) {
        let (state, entry) = self.modal(index);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Executed);
        assert_eq!(
            available_action(&state, &entry, member, i64::MAX),
            Err(NoAction::Finished)
        );
    }
}

/// Case 1 and 2: approved by others elsewhere, or by the acting member in an
/// earlier transaction; executed later by the acting member, with a hot
/// wallet that is no member paying.
#[test]
fn a_proposal_approved_earlier_is_executed_later_by_a_member_paying_or_not() {
    for program in BOTH {
        let mut chain = Chain::new();
        let [a, b, c] = [(); 3].map(|_| funded(&mut chain));
        let members = [a.pubkey(), b.pubkey(), c.pubkey()].map(|key| (key, PERMISSION_ALL));
        let team = Team::on(chain, program, &members, 2, 0, None);

        // Approved by the two others; the acting member never voted.
        team.propose(1, &[team.payment(SOL / 4)]);
        team.approve(1, &b);
        team.approve(1, &c);
        let (_, entry) = team.modal(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Approved, "{program:?}");
        assert!(!proposal.approved.contains(&a.pubkey()));
        team.execute_as(1, &a.pubkey(), &[&team.fees, &a]);
        team.assert_executed(1, &a.pubkey());
        assert_eq!(team.node.chain().balance(&team.recipient), SOL / 4);

        // Approved by the acting member and another, then executed by the
        // acting member in a transaction of its own that it pays for.
        team.propose(2, &[team.payment(SOL / 2)]);
        team.approve(2, &a);
        team.approve(2, &b);
        let (state, entry) = team.modal(2);
        assert_eq!(
            available_action(&state, &entry, &a.pubkey(), 0),
            Ok(ProposalAction::Execute),
            "{program:?}"
        );
        let request = ProposalRequest::Single {
            member: a.pubkey(),
            action: ProposalAction::Execute,
        };
        let wire = team.review(&team.build(2, &request, &a.pubkey()));
        let tx = decode_v1_transaction(&wire).unwrap();
        assert_eq!(
            tx.signers.len(),
            1,
            "{program:?}: the member pays and executes"
        );
        team.submit(&press_sign(Some(&team.node), &wire, &[&a]).wire);
        team.assert_executed(2, &a.pubkey());
        assert_eq!(team.node.chain().balance(&team.recipient), 3 * SOL / 4);
    }
}

/// Case 3: the time lock holds an approved proposal. Execute is not offered
/// before it ends, and the chain would refuse it; it is offered and lands
/// after.
#[test]
fn a_time_lock_holds_execution_until_it_ends() {
    for program in BOTH {
        let mut chain = Chain::new();
        let [a, b] = [(); 2].map(|_| funded(&mut chain));
        let members = [a.pubkey(), b.pubkey()].map(|key| (key, PERMISSION_ALL));
        let team = Team::on(chain, program, &members, 2, 3_600, None);
        team.propose(1, &[team.payment(SOL)]);
        team.approve(1, &b);
        // The last approval does not execute: the time lock is there.
        let (state, entry) = team.modal(1);
        assert_eq!(
            available_action(&state, &entry, &a.pubkey(), 0),
            Ok(ProposalAction::Approve),
            "{program:?}"
        );
        team.approve(1, &a);

        let (state, entry) = team.modal(1);
        let approved_at = entry.proposal.as_ref().unwrap().since;
        let now = team.node.chain().now();
        let verdict = available_action(&state, &entry, &a.pubkey(), now);
        assert_eq!(
            verdict,
            Err(NoAction::TimeLock {
                until: approved_at + 3_600,
                now
            }),
            "{program:?}"
        );
        assert_eq!(
            verdict.unwrap_err().to_string(),
            "It is approved. The multisig's time lock holds it for 60 minutes more, then it can be executed."
        );
        // Sent anyway, the program refuses it.
        let execute = team.squad.execute(1, 0, &[team.payment(SOL)], &a.pubkey());
        let refused = team.node.chain().send(&[&a], vec![execute]).unwrap_err();
        assert!(
            logs(&refused).contains("TimeLockNotReleased"),
            "{program:?}: {}",
            logs(&refused)
        );

        // A modal left open renders again at the end: by then it is offered.
        team.node.chain().pass_seconds(3_599);
        let now = team.node.chain().now();
        assert!(matches!(
            available_action(&state, &entry, &a.pubkey(), now),
            Err(NoAction::TimeLock { .. })
        ));
        team.node.chain().pass_seconds(1);
        team.execute_as(1, &a.pubkey(), &[&team.fees, &a]);
        team.assert_executed(1, &a.pubkey());
    }
}

/// Case 4: the multisig changed after the proposal was approved. Both
/// programs still execute an approved vault transaction that went stale; an
/// open one can no longer be voted on.
#[test]
fn an_approved_proposal_that_went_stale_is_still_executed() {
    for program in BOTH {
        let mut chain = Chain::new();
        let [a, b, authority] = [(); 3].map(|_| funded(&mut chain));
        let members = [a.pubkey(), b.pubkey()].map(|key| (key, PERMISSION_ALL));
        let team = Team::on(chain, program, &members, 2, 0, Some(authority.pubkey()));
        team.propose(1, &[team.payment(SOL / 4)]);
        team.approve(1, &a);
        team.approve(1, &b);
        team.propose(2, &[team.payment(SOL / 4)]);
        team.approve(2, &b);

        // The authority adds a member: everything before is stale.
        let newcomer = KeypairSigner::new_random().pubkey();
        let add = crate::squads_config::add_member(&team.squad, &authority.pubkey(), &newcomer, 7);
        let sent = team.node.chain().send(&[&authority], vec![add]);
        sent.unwrap_or_else(|failure| panic!("{program:?}: {}", logs(&failure)));

        let (state, open) = team.modal(2);
        assert!(open.stale, "{program:?}");
        assert_eq!(
            available_action(&state, &open, &a.pubkey(), 0),
            Err(NoAction::Stale)
        );
        let refused = team
            .node
            .chain()
            .send(&[&a], vec![team.squad.approve(2, &a.pubkey())]);
        assert!(
            refused.is_err(),
            "{program:?}: a stale proposal took a vote"
        );

        let (_, approved) = team.modal(1);
        assert!(approved.stale, "{program:?}");
        team.execute_as(1, &a.pubkey(), &[&team.fees, &a]);
        team.assert_executed(1, &a.pubkey());
        assert_eq!(team.node.chain().balance(&team.recipient), SOL / 4);
    }
}

/// Case 5: a member that may only execute executes; an acting member that
/// may only vote hands the execution to another wallet of this device that
/// may execute.
#[test]
fn execution_goes_to_a_wallet_of_this_device_that_may_execute() {
    for program in BOTH {
        let mut chain = Chain::new();
        let [voter, other, executor] = [(); 3].map(|_| funded(&mut chain));
        let members = [
            (voter.pubkey(), PERMISSION_VOTE),
            (other.pubkey(), PERMISSION_VOTE),
            (executor.pubkey(), PERMISSION_EXECUTE),
        ];
        let team = Team::on(chain, program, &members, 2, 0, None);
        team.propose(1, &[team.payment(SOL)]);
        team.approve(1, &voter);
        team.approve(1, &other);

        // The device holds the voter, acting, and the executor beside it.
        let hot = |label: &str, key: &KeypairSigner| {
            WalletRecord::from_keypair_seed(label, key.seed_bytes())
        };
        let (acting, beside) = (hot("Voter", &voter), hot("Executor", &executor));
        let mut multisig = WalletRecord::squads("Team", team.squad, Vec::new(), 2);
        multisig.kind.set_member_wallet(Some(acting.id));
        multisig.kind.set_approves_with(vec![beside.id]);
        let id = multisig.id;
        let wallets = vec![acting, beside, multisig];

        let (state, entry) = team.modal(1);
        assert_eq!(
            available_action(&state, &entry, &voter.pubkey(), 0),
            Err(NoAction::MayNotExecute),
            "{program:?}"
        );
        let chosen = chosen_members(&wallets, id, &state);
        let instead = executor_instead(&state, &entry, &chosen, 0).expect("an executor");
        assert_eq!(instead.address, executor.pubkey(), "{program:?}");
        assert_eq!(instead.kind, MemberKind::HotKey);
        team.execute_as(1, &executor.pubkey(), &[&team.fees, &executor]);
        team.assert_executed(1, &executor.pubkey());
    }
}

/// Case 6: a programmatic signer executes. Sign rebuilds the execution
/// around its authorization, which its authority signs on another device.
#[test]
fn a_programmatic_member_executes_through_its_authorization() {
    for program in BOTH {
        let mut chain = Chain::new();
        let fees = funded(&mut chain);
        let voter = funded(&mut chain);
        let authority = KeypairSigner::new_random();
        let mut signer = WalletRecord::programmatic("Signer", authority.pubkey(), None);
        let member = signer.address().unwrap();
        let (nonce, create_nonce) = create_executor_nonce(
            &fees.pubkey(),
            &fees.pubkey(),
            "airlock",
            &member,
            chain.nonce_rent(),
        )
        .unwrap();
        chain.send(&[&fees], create_nonce.to_vec()).unwrap();
        signer.kind.set_executor_nonce(Some(nonce));
        let members = [
            (voter.pubkey(), PERMISSION_ALL),
            (member, PERMISSION_EXECUTE),
        ];
        let team = Team::on(chain, program, &members, 1, 0, None);
        team.propose(1, &[team.payment(SOL)]);
        team.approve(1, &voter);

        let wallets = vec![
            WalletRecord::from_keypair_seed("Fees", fees.seed_bytes()),
            signer,
        ];
        let (state, entry) = team.modal(1);
        assert_eq!(
            available_action(&state, &entry, &member, 0),
            Ok(ProposalAction::Execute)
        );
        let request = ProposalRequest::Single {
            member,
            action: ProposalAction::Execute,
        };
        let built = team.build(1, &request, &fees.pubkey());
        let tx = decode_v1_transaction(&built).unwrap();
        let plan = plan_adaptation(&tx, &wallets, None)
            .unwrap_or_else(|err| panic!("{err}"))
            .expect("the programmatic signer is asked to sign");
        let (adapted, _) = run(wallet_flows::adapt::rebuild(&team.node, &tx, &plan))
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));

        // The authority's device returns its one signature.
        let cold = press_sign(None, &adapted.wire, &[&authority]);
        let objects = authority_signature_objects(&decode_v1_transaction(&cold.wire).unwrap());
        let QrObject::Signature(back) = carried(&objects[0].signed) else {
            panic!("a signature");
        };
        let (authorized, _) = accept_signature(&adapted.wire, &back).unwrap();
        let done = press_sign(Some(&team.node), &authorized, &[&fees]);
        team.submit(&done.wire);
        team.assert_executed(1, &member);
        assert_eq!(team.node.chain().balance(&team.recipient), SOL);
    }
}

/// Case 6: a Vector wallet executes. Sign rebuilds the execution around the
/// `Advance` its key signs.
#[test]
fn a_vector_member_executes_through_its_advance() {
    for program in BOTH {
        let mut desk = VectorDesk::new();
        let account = desk.add_vector("Quantum", true);
        let (squad, voter, recipient) = {
            let mut chain = desk.chain();
            let voter = funded(&mut chain);
            let members = [
                (voter.pubkey(), PERMISSION_ALL),
                (account.address, PERMISSION_EXECUTE),
            ];
            let squad = create(&mut chain, program, &voter, &members, 1, 0, None);
            let recipient = KeypairSigner::new_random().pubkey();
            let payment =
                solana_system_interface::instruction::transfer(&squad.vault(0), &recipient, SOL);
            let by = voter.pubkey();
            chain
                .send(
                    &[&voter],
                    vec![
                        squad
                            .create_transaction(1, 0, &[payment], &by, &by)
                            .unwrap(),
                        squad.create_proposal(1, &by, &by),
                        squad.approve(1, &by),
                    ],
                )
                .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
            (squad, voter, recipient)
        };
        let _ = voter;
        let (state, entries) =
            run(wallet_flows::squads::load_page(&desk.node, &squad, None)).unwrap();
        let entry = entries.into_iter().find(|entry| entry.index == 1).unwrap();
        assert_eq!(
            available_action(&state, &entry, &account.address, 0),
            Ok(ProposalAction::Execute),
            "{program:?}"
        );
        let request = ProposalRequest::Single {
            member: account.address,
            action: ProposalAction::Execute,
        };
        let instructions = request.instructions(&squad, &entry).unwrap();
        let (built, _) = run(compile_fresh(&desk.node, desk.payer.pubkey(), instructions)).unwrap();
        let adapted = desk.rebuild_wire(&built);
        let signed = desk
            .sign(&adapted.wire, &adapted.context)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        desk.land(&signed.wire);
        assert_eq!(desk.chain().balance(&recipient), SOL, "{program:?}");
    }
}

/// Case 6: an offline key executes. On a blockhash the trip to its device
/// outlives the transaction; moved onto the key's durable nonce, as the
/// Proposals modal does for such a member, it lands however long it took.
#[test]
fn an_offline_member_executes_on_its_durable_nonce() {
    for program in BOTH {
        let mut chain = Chain::new();
        let voter = funded(&mut chain);
        let cold = KeypairSigner::new_random();
        let members = [
            (voter.pubkey(), PERMISSION_ALL),
            (cold.pubkey(), PERMISSION_ALL),
        ];
        let team = Team::on(chain, program, &members, 1, 0, None);
        let nonce_key = KeypairSigner::new_random();
        let nonce = nonce_key.pubkey();
        {
            let mut chain = team.node.chain();
            let rent = chain.rent(80);
            let create = solana_system_interface::instruction::create_nonce_account(
                &team.fees.pubkey(),
                &nonce,
                &cold.pubkey(),
                rent,
            );
            chain.send(&[&team.fees, &nonce_key], create).unwrap();
        }
        team.propose(1, &[team.payment(SOL)]);
        team.approve(1, &voter);

        let request = ProposalRequest::Single {
            member: cold.pubkey(),
            action: ProposalAction::Execute,
        };
        let built = team.build(1, &request, &team.fees.pubkey());
        let tx = decode_v1_transaction(&built).unwrap();
        let moved = run(wallet_flows::offline_nonce::move_onto(
            &team.node,
            &tx,
            cold.pubkey(),
            nonce,
        ))
        .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        let moved_tx = decode_v1_transaction(&moved).unwrap();
        assert!(moved_tx.uses_durable_nonce, "{program:?}");
        let paid = press_sign(Some(&team.node), &moved, &[&team.fees]);
        // Much later, on the other device.
        team.node.chain().age_blockhashes(200);
        let cold_signed = press_sign(None, &moved, &[&cold]);
        let objects =
            wallet_core::signature_objects(&decode_v1_transaction(&cold_signed.wire).unwrap());
        let QrObject::Signature(back) = carried(&objects[0].signed) else {
            panic!("a signature");
        };
        let (wire, _) = accept_signature(&paid.wire, &back).unwrap();
        team.submit(&wire);
        team.assert_executed(1, &cold.pubkey());
    }
}

/// Case 7: a long stored transaction. The programs build every instruction
/// on the heap before running them, and past about fifty the default heap
/// runs out: compiled plainly, the execution fails with an access violation.
/// The modal asks for the most heap for an execution, and Review and Sign
/// fits the compute limit to what the simulation used; it lands.
#[test]
fn a_long_execution_gets_the_heap_and_compute_it_needs() {
    for program in BOTH {
        let mut chain = Chain::new();
        let a = funded(&mut chain);
        let team = Team::on(chain, program, &[(a.pubkey(), PERMISSION_ALL)], 1, 0, None);
        let payments: Vec<Instruction> = (0..PAYMENTS).map(|_| team.payment(1_000_000)).collect();
        let by = team.proposer.pubkey();
        let store = team
            .squad
            .create_transaction(1, 0, &payments, &by, &by)
            .unwrap();
        let open = team.squad.create_proposal(1, &by, &by);
        let budget = wallet_core::ComputeBudgetSpec {
            cu_limit: Some(1_400_000),
            ..Default::default()
        };
        let sent = team
            .node
            .chain()
            .send_with(&[&team.proposer], vec![store, open], budget);
        sent.unwrap_or_else(|failure| panic!("{program:?}: {}", logs(&failure)));
        team.approve(1, &a);

        let request = ProposalRequest::Single {
            member: a.pubkey(),
            action: ProposalAction::Execute,
        };
        // As the modal compiled it before: the default heap.
        let (_, entry) = team.modal(1);
        let instructions = request.instructions(&team.squad, &entry).unwrap();
        let (plain, _) = run(compile_fresh(&team.node, team.fees.pubkey(), instructions)).unwrap();
        let signed = press_sign(None, &plain, &[&team.fees, &a]);
        let failed = team.node.chain().send_wire(&signed.wire).unwrap_err();
        assert!(
            logs(&failed).contains("Access violation"),
            "{program:?}: {}",
            logs(&failed)
        );

        let built = team.build(1, &request, &team.fees.pubkey());
        let tx = decode_v1_transaction(&built).unwrap();
        assert_eq!(tx.budget.heap_size, Some(wallet_core::relay::MAX_HEAP_SIZE));
        let fitted = team.review(&built);
        let limit = decode_v1_transaction(&fitted).unwrap().stated_cu_limit();
        // More than the 200,000 the one instruction gets unless told.
        assert!(limit > 200_000, "{program:?}: {limit}");
        team.submit(&press_sign(Some(&team.node), &fitted, &[&team.fees, &a]).wire);
        team.assert_executed(1, &a.pubkey());
        assert_eq!(
            team.node.chain().balance(&team.recipient),
            PAYMENTS * 1_000_000
        );
        println!("{program:?}: {PAYMENTS} payments executed within {limit} compute units");
    }
}

/// Case 7: a stored transaction that reads more accounts from lookup tables
/// than one transaction can list. It can be read and approved, and its
/// execution does not compile: the modal says so instead.
#[test]
fn an_execution_listing_more_accounts_than_a_transaction_holds_does_not_compile() {
    for program in BOTH {
        let mut chain = Chain::new();
        let a = funded(&mut chain);
        let recipients: Vec<Pubkey> = (0..130)
            .map(|_| KeypairSigner::new_random().pubkey())
            .collect();
        let table = chain.plant_lookup_table(&recipients);
        let team = Team::on(chain, program, &[(a.pubkey(), PERMISSION_ALL)], 1, 0, None);
        // One instruction that names all of them: small to store, and too
        // many accounts for one transaction to list.
        let mut accounts = vec![solana_instruction::AccountMeta::new(team.vault, true)];
        accounts.extend(
            recipients
                .iter()
                .map(|to| solana_instruction::AccountMeta::new_readonly(*to, false)),
        );
        let payments = vec![Instruction {
            program_id: wallet_core::MEMO_PROGRAM_ID,
            accounts,
            data: b"all of them".to_vec(),
        }];
        let by = team.proposer.pubkey();
        let store = team
            .squad
            .create_transaction_with_tables(
                1,
                0,
                &payments,
                &[(table, recipients.clone())],
                0,
                &by,
                &by,
            )
            .unwrap();
        team.propose_with(1, store);

        let (state, entry) = team.modal(1);
        let Stored::Vault(stored) = &entry.stored else {
            panic!("{program:?}: {:?}", entry.stored);
        };
        assert_eq!(stored.instructions, payments, "{program:?}");
        assert_eq!(
            available_action(&state, &entry, &a.pubkey(), 0),
            Ok(ProposalAction::ApproveAndExecute)
        );
        let request = ProposalRequest::Single {
            member: a.pubkey(),
            action: ProposalAction::Approve,
        };
        let approve = request.instructions(&team.squad, &entry).unwrap();
        assert!(
            wallet_core::compile_v1(a.pubkey(), approve, None, Some(Default::default())).is_ok()
        );
        let execute = ProposalRequest::Single {
            member: a.pubkey(),
            action: ProposalAction::ApproveAndExecute,
        }
        .instructions(&team.squad, &entry)
        .unwrap();
        let refused = wallet_core::compile_v1(a.pubkey(), execute, None, Some(Default::default()));
        assert!(
            matches!(refused, Err(BuildError::TooLarge { len }) if len > wallet_core::MAX_TRANSACTION_SIZE),
            "{program:?}: {refused:?}"
        );
    }
}

/// Case 8: a stored transaction with an ephemeral signer, an address the
/// program derives for it and signs as. The execution lists it as not
/// signing, as both programs take it, and it lands.
#[test]
fn an_ephemeral_signer_is_signed_for_by_the_program() {
    for program in BOTH {
        let mut chain = Chain::new();
        let a = funded(&mut chain);
        let team = Team::on(chain, program, &[(a.pubkey(), PERMISSION_ALL)], 1, 0, None);
        let fresh = team.squad.ephemeral_signer(1, 0);
        let rent = team.node.chain().rent(0);
        let create = solana_system_interface::instruction::create_account(
            &team.vault,
            &fresh,
            rent,
            0,
            &wallet_core::SYSTEM_PROGRAM_ID,
        );
        let by = team.proposer.pubkey();
        let store = team
            .squad
            .create_transaction_with_tables(1, 0, std::slice::from_ref(&create), &[], 1, &by, &by)
            .unwrap();
        team.propose_with(1, store);
        team.approve(1, &a);

        let (_, entry) = team.modal(1);
        let Stored::Vault(stored) = &entry.stored else {
            panic!("{program:?}: {:?}", entry.stored);
        };
        assert_eq!(stored.ephemeral_signers, 1);
        assert_eq!(stored.instructions, vec![create], "{program:?}");
        assert!(stored.other_signers.is_empty(), "{program:?}");
        let execute = team.squad.execute_stored(stored, &a.pubkey());
        assert!(
            execute
                .accounts
                .iter()
                .all(|meta| !meta.is_signer || meta.pubkey == a.pubkey()),
            "{program:?}: only the member signs"
        );
        team.execute_as(1, &a.pubkey(), &[&team.fees, &a]);
        team.assert_executed(1, &a.pubkey());
        assert_eq!(team.node.chain().balance(&fresh), rent, "{program:?}");
    }
}

/// Case 9: a proposal whose stored transaction reads accounts from lookup
/// tables, as other apps store swaps. The modal reads the tables, shows its
/// instructions, offers Approve and Execute; Review and Sign reads what it
/// runs; it lands.
#[test]
fn a_proposal_that_reads_lookup_tables_is_read_approved_and_executed() {
    for program in BOTH {
        let mut chain = Chain::new();
        let [a, b] = [(); 2].map(|_| funded(&mut chain));
        let (first, second) = (
            KeypairSigner::new_random().pubkey(),
            KeypairSigner::new_random().pubkey(),
        );
        let filler = KeypairSigner::new_random().pubkey();
        let one = chain.plant_lookup_table(&[filler, first]);
        let other = chain.plant_lookup_table(&[second]);
        let members = [a.pubkey(), b.pubkey()].map(|key| (key, PERMISSION_ALL));
        let team = Team::on(chain, program, &members, 2, 0, None);
        let payments = vec![
            solana_system_interface::instruction::transfer(&team.vault, &first, SOL / 4),
            solana_system_interface::instruction::transfer(&team.vault, &second, SOL / 2),
        ];
        let by = team.proposer.pubkey();
        let tables = [(one, vec![filler, first]), (other, vec![second])];
        let store = team
            .squad
            .create_transaction_with_tables(1, 0, &payments, &tables, 0, &by, &by)
            .unwrap();
        team.propose_with(1, store);
        team.approve(1, &b);

        let (state, entry) = team.modal(1);
        let Stored::Vault(stored) = &entry.stored else {
            panic!("{program:?}: {:?}", entry.stored);
        };
        assert_eq!(stored.instructions, payments, "{program:?}");
        assert_eq!(stored.lookup_tables, vec![one, other]);
        assert!(stored.deactivated_tables.is_empty());
        assert_eq!(
            available_action(&state, &entry, &a.pubkey(), 0),
            Ok(ProposalAction::ApproveAndExecute),
            "{program:?}"
        );

        let request = ProposalRequest::Single {
            member: a.pubkey(),
            action: ProposalAction::ApproveAndExecute,
        };
        let wire = team.review(&team.build(1, &request, &team.fees.pubkey()));
        // Review and Sign reads what the execution runs, tables and all.
        let target = StoredTarget {
            squad: team.squad,
            proposal: team.squad.proposal(1),
            transaction: Some(team.squad.transaction(1)),
        };
        let behind = run(wallet_flows::squads::read_stored(&team.node, &[target])).unwrap();
        let shown = stored_in(&target, &behind).expect("what it runs is read");
        assert_eq!(shown.instructions, payments, "{program:?}");
        assert!(behind.contains_key(&one) && behind.contains_key(&other));

        team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees, &a]).wire);
        team.assert_executed(1, &a.pubkey());
        let chain = team.node.chain();
        assert_eq!(chain.balance(&first), SOL / 4, "{program:?}");
        assert_eq!(chain.balance(&second), SOL / 2, "{program:?}");
    }
}

/// Case 9: what a table's state does to such a proposal. Deactivated, it is
/// still read and executed, and the modal says it may be closed. Closed, it
/// cannot be read, and the program refuses the execution too. Holding fewer
/// entries than the message names, what it would run is not known.
#[test]
fn a_deactivated_table_still_executes_and_a_closed_or_short_one_is_told() {
    for program in BOTH {
        let mut chain = Chain::new();
        let a = funded(&mut chain);
        let to = KeypairSigner::new_random().pubkey();
        let table = chain.plant_lookup_table(&[to]);
        let team = Team::on(chain, program, &[(a.pubkey(), PERMISSION_ALL)], 1, 0, None);
        let payment = solana_system_interface::instruction::transfer(&team.vault, &to, SOL);
        let by = team.proposer.pubkey();
        for index in [1, 2] {
            let store = team
                .squad
                .create_transaction_with_tables(
                    index,
                    0,
                    std::slice::from_ref(&payment),
                    &[(table, vec![to])],
                    0,
                    &by,
                    &by,
                )
                .unwrap();
            team.propose_with(index, store);
            team.approve(index, &a);
        }

        // Its authority deactivates it: still read, still executed.
        team.node.chain().patch_data(&table, |data| {
            data[4..12].copy_from_slice(&5u64.to_le_bytes());
        });
        let (_, entry) = team.modal(1);
        let Stored::Vault(stored) = &entry.stored else {
            panic!("{program:?}: {:?}", entry.stored);
        };
        assert_eq!(stored.deactivated_tables, vec![table], "{program:?}");
        let held = stored.clone();
        team.execute_as(1, &a.pubkey(), &[&team.fees, &a]);
        team.assert_executed(1, &a.pubkey());

        // Then it is closed: nothing can be read or executed.
        team.node.chain().remove(&table);
        let (state, entry) = team.modal(2);
        assert_eq!(
            entry.stored,
            Stored::LookupTables(TableProblem::Missing { table }),
            "{program:?}"
        );
        assert_eq!(
            available_action(&state, &entry, &a.pubkey(), 0),
            Err(NoAction::Unreadable)
        );
        // Built from what was read before, the program refuses it.
        let mut stale_read = held;
        stale_read.index = 2;
        let execute = team.squad.execute_stored(&stale_read, &a.pubkey());
        let refused = team.node.chain().send(&[&a], vec![execute]);
        assert!(
            refused.is_err(),
            "{program:?}: an execution without its table landed"
        );

        // A table that holds fewer entries than the message names.
        let mut chain = team.node.chain();
        let short = chain.plant_lookup_table(&[]);
        drop(chain);
        let store = team
            .squad
            .create_transaction_with_tables(
                3,
                0,
                std::slice::from_ref(&payment),
                &[(short, vec![to])],
                0,
                &by,
                &by,
            )
            .unwrap();
        team.propose_with(3, store);
        let (_, entry) = team.modal(3);
        assert_eq!(
            entry.stored,
            Stored::LookupTables(TableProblem::Short {
                table: short,
                index: 0,
                holds: 0
            }),
            "{program:?}"
        );
    }
}

/// Partly executed is not a state either program puts a proposal in any
/// more: `Executing` is deprecated in both, and a batch partly executed
/// stays approved. The page is read again after an execution lands, and the
/// rows that are over offer nothing.
#[test]
fn an_executed_proposal_reads_as_executed_with_nothing_left_to_do() {
    let mut chain = Chain::new();
    let a = funded(&mut chain);
    let team = Team::on(
        chain,
        SquadsProgram::V4,
        &[(a.pubkey(), PERMISSION_ALL)],
        1,
        0,
        None,
    );
    team.propose(1, &[team.payment(SOL)]);
    let (state, entry) = team.modal(1);
    assert_eq!(
        available_action(&state, &entry, &a.pubkey(), 0),
        Ok(ProposalAction::ApproveAndExecute)
    );
    let request = ProposalRequest::Single {
        member: a.pubkey(),
        action: ProposalAction::ApproveAndExecute,
    };
    let wire = team.review(&team.build(1, &request, &team.fees.pubkey()));
    team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees, &a]).wire);
    team.assert_executed(1, &a.pubkey());
    let mut by_index: HashMap<u64, ProposalStatus> = HashMap::new();
    let (_, entries) = run(wallet_flows::squads::load_page(
        &team.node,
        &team.squad,
        None,
    ))
    .unwrap();
    for entry in entries {
        by_index.insert(entry.index, entry.proposal.unwrap().status);
    }
    assert_eq!(by_index, HashMap::from([(1, ProposalStatus::Executed)]));
}
