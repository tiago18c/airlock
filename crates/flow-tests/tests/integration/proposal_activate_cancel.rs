//! Opening a draft for votes and voting to cancel an approved proposal, on
//! the path a person takes, against the real programs: the Proposals modal
//! reads the page and offers the action, it is compiled as the modal does,
//! Review and Sign simulates and fits it, the keys sign, it is submitted, and
//! the modal, opened again, reads the proposal as the program left it.
//!
//! What the programs allow, proven here on both:
//!
//! - Any member with the initiate permission opens a draft, not only the one
//!   that made it; a member without it is refused (`Unauthorized`), and so
//!   is a stale draft (`StaleProposal`).
//! - A draft cannot be cancelled, rejected or approved
//!   (`InvalidProposalStatus`): opening it is the only thing a member can do
//!   to it.
//! - A cancel vote is taken on an approved proposal only, from a member with
//!   the vote permission, also one that approved it, also while the time
//!   lock holds it; it is cancelled once the members' cancel votes reach the
//!   threshold, a second vote by the same member is refused
//!   (`AlreadyCancelled`), and a cancelled proposal is never executed.
//! - The cancel vote of a key that left the multisig is dropped before
//!   counting.
//! - A cancel vote grows the proposal account when the multisig has more
//!   members than when it was made, and the member pays for it.

use crate::proposal_execute::{create, funded, press_sign, run, Team};
use crate::squads_config::{add_member, remove_member};
use crate::support::vector::Desk as VectorDesk;
use crate::support::{carried, logs, Chain, SOL};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use wallet_core::adapt::plan_adaptation;
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::proposals::{
    available_action, available_cancel, member_choices, CancelVote, NoAction, ProposalAction,
    ProposalRequest,
};
use wallet_core::protocol::QrObject;
use wallet_core::relay::{accept_signature, authority_signature_objects};
use wallet_core::squads::{
    ProposalStatus, Squad, SquadsProgram, PERMISSION_ALL, PERMISSION_EXECUTE, PERMISSION_INITIATE,
    PERMISSION_VOTE,
};
use wallet_core::{decode_v1_transaction, KeypairSigner, WalletRecord, WalletSigner};
use wallet_flows::compile::compile_fresh;

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

fn single(member: Pubkey, action: ProposalAction) -> ProposalRequest {
    ProposalRequest::Single { member, action }
}

/// Three members that may do everything, 2 of 3, and the proposer, which may
/// only propose. `authority` makes it a controlled multisig.
fn three(
    program: SquadsProgram,
    time_lock: u32,
    authority: Option<Pubkey>,
) -> (Team, [KeypairSigner; 3]) {
    let mut chain = Chain::new();
    let keys = [(); 3].map(|_| funded(&mut chain));
    let members =
        [keys[0].pubkey(), keys[1].pubkey(), keys[2].pubkey()].map(|key| (key, PERMISSION_ALL));
    (
        Team::on(chain, program, &members, 2, time_lock, authority),
        keys,
    )
}

/// The program refused `instructions`, signed by `keys`, with `error`.
fn refused(team: &Team, keys: &[&KeypairSigner], instructions: Vec<Instruction>, error: &str) {
    let failure = team.node.chain().send(keys, instructions).unwrap_err();
    let logs = logs(&failure);
    assert!(logs.contains(error), "{:?}: {logs}", team.squad.program);
}

impl Team {
    /// The proposer stores a payment of `lamports` as transaction `index`
    /// and opens a draft proposal for it.
    fn draft(&self, index: u64, lamports: u64) {
        let by = self.proposer.pubkey();
        let store = self
            .squad
            .create_transaction(index, 0, &[self.payment(lamports)], &by, &by)
            .unwrap();
        let draft = self.squad.create_draft(index, &by, &by);
        let sent = self
            .node
            .chain()
            .send(&[&self.proposer], vec![store, draft]);
        sent.unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    }

    /// `member` does `action` to `index` as the modal offers it, with the
    /// hot wallet that is no member paying, all the way to the chain.
    fn act(&self, index: u64, member: &KeypairSigner, action: ProposalAction) {
        let wire =
            self.review(&self.build(index, &single(member.pubkey(), action), &self.fees.pubkey()));
        let signed = press_sign(Some(&self.node), &wire, &[&self.fees, member]);
        self.submit(&signed.wire);
    }

    fn status(&self, index: u64) -> ProposalStatus {
        self.modal(index).1.proposal.unwrap().status
    }
}

/// A draft is opened by a member that did not make it, then approved and
/// executed. Before it is opened, the programs take neither a cancel vote,
/// a rejection nor an approval: there is no cancelling a draft.
#[test]
fn a_draft_is_opened_then_approved_and_executed_and_cannot_be_cancelled() {
    for program in BOTH {
        let (team, [a, b, _]) = three(program, 0, None);
        team.draft(1, SOL);
        let (state, entry) = team.modal(1);
        assert_eq!(
            entry.proposal.as_ref().unwrap().status,
            ProposalStatus::Draft
        );
        let choices = member_choices(&state, &entry, &a.pubkey(), 0);
        assert_eq!(choices.forward, Ok(ProposalAction::Activate), "{program:?}");
        assert_eq!(choices.turn_down(), None);

        // What the app does not offer, the programs do not take.
        for vote in [
            team.squad.cancel(1, &a.pubkey()),
            team.squad.reject(1, &a.pubkey()),
            team.squad.approve(1, &a.pubkey()),
        ] {
            refused(&team, &[&a], vec![vote], "InvalidProposalStatus");
        }

        // Opening it is the member and the fee payer signing, nothing more.
        let built = team.build(
            1,
            &single(a.pubkey(), ProposalAction::Activate),
            &team.fees.pubkey(),
        );
        assert_eq!(decode_v1_transaction(&built).unwrap().signers.len(), 2);
        team.act(1, &a, ProposalAction::Activate);
        let (state, entry) = team.modal(1);
        assert_eq!(
            entry.proposal.as_ref().unwrap().status,
            ProposalStatus::Active,
            "{program:?}"
        );
        assert!(entry.proposal.as_ref().unwrap().approved.is_empty());
        // Open now: approve and reject, not open again.
        let choices = member_choices(&state, &entry, &a.pubkey(), 0);
        assert_eq!(choices.forward, Ok(ProposalAction::Approve));
        assert!(choices.reject.is_some());
        refused(
            &team,
            &[&a],
            vec![team.squad.activate(1, &a.pubkey())],
            "InvalidProposalStatus",
        );

        team.approve(1, &a);
        let (state, entry) = team.modal(1);
        assert_eq!(
            available_action(&state, &entry, &b.pubkey(), 0),
            Ok(ProposalAction::ApproveAndExecute)
        );
        team.act(1, &b, ProposalAction::ApproveAndExecute);
        team.assert_executed(1, &b.pubkey());
        assert_eq!(team.node.chain().balance(&team.recipient), SOL);
    }
}

/// Opening a draft is part of proposing: a member with only the initiate
/// permission may, one with vote and execute but not initiate may not.
#[test]
fn only_a_member_that_may_propose_opens_a_draft() {
    for program in BOTH {
        let mut chain = Chain::new();
        let voter = funded(&mut chain);
        let initiator = funded(&mut chain);
        let members = [
            (voter.pubkey(), PERMISSION_VOTE | PERMISSION_EXECUTE),
            (initiator.pubkey(), PERMISSION_INITIATE),
        ];
        let team = Team::on(chain, program, &members, 1, 0, None);
        team.draft(1, SOL);

        let (state, entry) = team.modal(1);
        assert_eq!(
            available_action(&state, &entry, &voter.pubkey(), 0),
            Err(NoAction::Draft),
            "{program:?}"
        );
        refused(
            &team,
            &[&voter],
            vec![team.squad.activate(1, &voter.pubkey())],
            "Unauthorized",
        );
        // Another proposer than the one that made it.
        assert_eq!(
            available_action(&state, &entry, &initiator.pubkey(), 0),
            Ok(ProposalAction::Activate)
        );
        team.act(1, &initiator, ProposalAction::Activate);
        assert_eq!(team.status(1), ProposalStatus::Active, "{program:?}");
    }
}

/// A change to the multisig makes a draft stale, and a stale draft can no
/// longer be opened.
#[test]
fn a_stale_draft_can_no_longer_be_opened() {
    for program in BOTH {
        let authority = KeypairSigner::new_random();
        let (team, [a, _, _]) = three(program, 0, Some(authority.pubkey()));
        team.node.chain().fund(&authority.pubkey(), SOL);
        team.draft(1, SOL);
        let newcomer = KeypairSigner::new_random().pubkey();
        let change = add_member(&team.squad, &authority.pubkey(), &newcomer, PERMISSION_ALL);
        team.node
            .chain()
            .send(&[&authority], vec![change])
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));

        let (state, entry) = team.modal(1);
        assert!(entry.stale, "{program:?}");
        let choices = member_choices(&state, &entry, &a.pubkey(), 0);
        assert_eq!(choices.forward, Err(NoAction::StaleDraft));
        assert_eq!(choices.turn_down(), None);
        refused(
            &team,
            &[&a],
            vec![team.squad.activate(1, &a.pubkey())],
            "StaleProposal",
        );
    }
}

/// Approved by two; one of them votes to cancel, which leaves it approved
/// and executable; a second vote by the same member is refused; the other
/// approver's cancel vote reaches the threshold, and it is cancelled for
/// good: nothing executes it.
#[test]
fn cancel_votes_that_reach_the_threshold_cancel_for_good() {
    for program in BOTH {
        let (team, [a, b, c]) = three(program, 0, None);
        team.propose(1, &[team.payment(SOL)]);
        team.approve(1, &a);
        team.approve(1, &b);

        let (state, entry) = team.modal(1);
        let choices = member_choices(&state, &entry, &a.pubkey(), 0);
        assert_eq!(choices.forward, Ok(ProposalAction::Execute), "{program:?}");
        assert_eq!(choices.cancel, Some(CancelVote { missing_after: 1 }));
        assert_eq!(choices.reject, None);
        // It runs nothing: the member and the fee payer sign it.
        let built = team.build(
            1,
            &single(a.pubkey(), ProposalAction::Cancel),
            &team.fees.pubkey(),
        );
        assert_eq!(decode_v1_transaction(&built).unwrap().signers.len(), 2);
        team.act(1, &a, ProposalAction::Cancel);

        let (state, entry) = team.modal(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Approved, "{program:?}");
        assert_eq!(proposal.cancelled, vec![a.pubkey()]);
        let mine = member_choices(&state, &entry, &a.pubkey(), 0);
        assert_eq!(mine.cancel, None);
        assert_eq!(mine.forward, Ok(ProposalAction::Execute));
        refused(
            &team,
            &[&a],
            vec![team.squad.cancel(1, &a.pubkey())],
            "AlreadyCancelled",
        );
        // The member that did not approve may vote to cancel too.
        assert!(available_cancel(&state, &entry, &c.pubkey())
            .unwrap()
            .last());
        let last = available_cancel(&state, &entry, &b.pubkey()).unwrap();
        assert!(last.last(), "{program:?}");
        team.act(1, &b, ProposalAction::Cancel);

        let (state, entry) = team.modal(1);
        assert_eq!(
            entry.proposal.as_ref().unwrap().status,
            ProposalStatus::Cancelled,
            "{program:?}"
        );
        for member in [&a, &b, &c] {
            let choices = member_choices(&state, &entry, &member.pubkey(), 0);
            assert_eq!(choices.forward, Err(NoAction::Cancelled));
            assert_eq!(choices.turn_down(), None);
        }
        let execute = team.squad.execute(1, 0, &[team.payment(SOL)], &c.pubkey());
        refused(&team, &[&c], vec![execute], "InvalidProposalStatus");
        refused(
            &team,
            &[&c],
            vec![team.squad.cancel(1, &c.pubkey())],
            "InvalidProposalStatus",
        );
        assert_eq!(team.node.chain().balance(&team.recipient), 0);
    }
}

/// While the time lock holds an approved proposal, Execute waits and the
/// members cancel it, which is what the lock leaves time for. Once the lock
/// has passed, it still cannot be executed.
#[test]
fn a_proposal_is_cancelled_while_its_time_lock_holds_it() {
    for program in BOTH {
        let (team, [a, b, c]) = three(program, 3_600, None);
        team.propose(1, &[team.payment(SOL)]);
        team.approve(1, &a);
        team.approve(1, &b);

        let (state, entry) = team.modal(1);
        let now = team.node.chain().now();
        let choices = member_choices(&state, &entry, &c.pubkey(), now);
        assert!(
            matches!(choices.forward, Err(NoAction::TimeLock { .. })),
            "{program:?}: {:?}",
            choices.forward
        );
        assert_eq!(choices.cancel, Some(CancelVote { missing_after: 1 }));
        team.act(1, &c, ProposalAction::Cancel);
        team.act(1, &a, ProposalAction::Cancel);
        assert_eq!(team.status(1), ProposalStatus::Cancelled, "{program:?}");

        team.node.chain().pass_seconds(3_601);
        let execute = team.squad.execute(1, 0, &[team.payment(SOL)], &a.pubkey());
        refused(&team, &[&a], vec![execute], "InvalidProposalStatus");
    }
}

/// A member votes to cancel and is then taken out of the multisig by its
/// authority. Both programs drop that vote before counting, so the next
/// member's cancel vote is the first that counts, and the proposal, stale
/// now, stays approved; one more cancels it.
#[test]
fn the_cancel_vote_of_a_member_taken_out_since_does_not_count() {
    for program in BOTH {
        let authority = KeypairSigner::new_random();
        let mut chain = Chain::new();
        chain.fund(&authority.pubkey(), SOL);
        let keys = [(); 4].map(|_| funded(&mut chain));
        let members = keys.each_ref().map(|key| (key.pubkey(), PERMISSION_ALL));
        let team = Team::on(chain, program, &members, 2, 0, Some(authority.pubkey()));
        let [a, b, c, gone] = &keys;
        team.propose(1, &[team.payment(SOL)]);
        team.approve(1, a);
        team.approve(1, b);
        team.act(1, gone, ProposalAction::Cancel);
        team.node
            .chain()
            .send(
                &[&authority],
                vec![remove_member(
                    &team.squad,
                    &authority.pubkey(),
                    &gone.pubkey(),
                )],
            )
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));

        let (state, entry) = team.modal(1);
        assert!(entry.stale, "{program:?}");
        // Listed, but not counted.
        assert_eq!(
            entry.proposal.as_ref().unwrap().cancelled,
            vec![gone.pubkey()]
        );
        assert_eq!(
            available_cancel(&state, &entry, &c.pubkey()),
            Some(CancelVote { missing_after: 1 }),
            "{program:?}"
        );
        team.act(1, c, ProposalAction::Cancel);
        let (_, entry) = team.modal(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Approved, "{program:?}");
        assert_eq!(proposal.cancelled, vec![c.pubkey()]);
        team.act(1, a, ProposalAction::Cancel);
        assert_eq!(team.status(1), ProposalStatus::Cancelled, "{program:?}");
    }
}

/// A member added after a proposal was made: the proposal account is grown
/// to fit every member's vote by the first cancel vote, and the member that
/// casts it pays the rent of the growth.
#[test]
fn a_cancel_vote_grows_the_proposal_and_the_member_pays() {
    for program in BOTH {
        let authority = KeypairSigner::new_random();
        let (team, [a, b, _]) = three(program, 0, Some(authority.pubkey()));
        team.node.chain().fund(&authority.pubkey(), SOL);
        team.propose(1, &[team.payment(SOL)]);
        team.approve(1, &a);
        team.approve(1, &b);
        let size = |team: &Team| team.node.chain().data(&team.squad.proposal(1)).len();
        let before = size(&team);
        let newcomer = KeypairSigner::new_random().pubkey();
        team.node
            .chain()
            .send(
                &[&authority],
                vec![add_member(
                    &team.squad,
                    &authority.pubkey(),
                    &newcomer,
                    PERMISSION_ALL,
                )],
            )
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));

        let paid = team.node.chain().balance(&a.pubkey());
        team.act(1, &a, ProposalAction::Cancel);
        // Three vote lists, one more member each.
        assert_eq!(size(&team), before + 3 * 32, "{program:?}");
        let after = size(&team);
        // One guard at a time: the chain is behind a mutex.
        let grown = team.node.chain().rent(after);
        let growth = grown - team.node.chain().rent(before);
        assert_eq!(
            paid - team.node.chain().balance(&a.pubkey()),
            growth,
            "{program:?}: the member pays the rent of the growth, the fee payer the fee"
        );
    }
}

/// A programmatic signer that may vote cancels, through its authorization,
/// which its authority signs on another device; once with the proposal as
/// big as it needs, and once when the cancel vote grows it, which the
/// signer's own address pays for. `fund_member` gives that address the
/// lamports, `None` leaves it empty.
fn programmatic_cancel(program: SquadsProgram, grow: bool, fund_member: Option<u64>) {
    let mut chain = Chain::new();
    let fees = funded(&mut chain);
    let voter = funded(&mut chain);
    let other = funded(&mut chain);
    let authority = KeypairSigner::new_random();
    let config = funded(&mut chain);
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
    if let Some(lamports) = fund_member {
        chain.fund(&member, lamports);
    }
    let members = [
        (voter.pubkey(), PERMISSION_ALL),
        (other.pubkey(), PERMISSION_ALL),
        (member, PERMISSION_VOTE),
    ];
    let team = Team::on(chain, program, &members, 2, 0, Some(config.pubkey()));
    team.propose(1, &[team.payment(SOL)]);
    team.approve(1, &voter);
    team.approve(1, &other);
    if grow {
        let newcomer = KeypairSigner::new_random().pubkey();
        team.node
            .chain()
            .send(
                &[&config],
                vec![add_member(
                    &team.squad,
                    &config.pubkey(),
                    &newcomer,
                    PERMISSION_ALL,
                )],
            )
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    }

    let wallets = vec![
        WalletRecord::from_keypair_seed("Fees", fees.seed_bytes()),
        signer,
    ];
    let (state, entry) = team.modal(1);
    assert_eq!(
        available_cancel(&state, &entry, &member),
        Some(CancelVote { missing_after: 1 }),
        "{program:?}"
    );
    let built = team.build(1, &single(member, ProposalAction::Cancel), &fees.pubkey());
    let tx = decode_v1_transaction(&built).unwrap();
    let plan = plan_adaptation(&tx, &wallets, None)
        .unwrap_or_else(|err| panic!("{err}"))
        .expect("the programmatic signer is asked to sign");
    let (adapted, _) = run(wallet_flows::adapt::rebuild(&team.node, &tx, &plan))
        .unwrap_or_else(|err| panic!("{program:?}: {err}"));

    let cold = press_sign(None, &adapted.wire, &[&authority]);
    let objects = authority_signature_objects(&decode_v1_transaction(&cold.wire).unwrap());
    let QrObject::Signature(back) = carried(&objects[0].signed) else {
        panic!("a signature");
    };
    let (authorized, _) = accept_signature(&adapted.wire, &back).unwrap();
    let done = press_sign(Some(&team.node), &authorized, &[&fees]);
    team.submit(&done.wire);

    let (_, entry) = team.modal(1);
    let proposal = entry.proposal.as_ref().unwrap();
    assert_eq!(proposal.cancelled, vec![member], "{program:?}");
    assert_eq!(proposal.status, ProposalStatus::Approved);
}

#[test]
fn a_programmatic_member_votes_to_cancel_through_its_authorization() {
    for program in BOTH {
        programmatic_cancel(program, false, None);
    }
}

#[test]
fn a_programmatic_member_pays_for_the_growth_from_its_own_address() {
    for program in BOTH {
        programmatic_cancel(program, true, Some(SOL / 10));
    }
}

/// Its address empty, a programmatic signer cannot pay for the growth: the
/// system program refuses the transfer, which is what the simulation on
/// Review and Sign shows, and the cancel vote does not land.
#[test]
fn a_programmatic_member_with_an_empty_address_cannot_pay_for_the_growth() {
    for program in BOTH {
        let failed = std::panic::catch_unwind(|| programmatic_cancel(program, true, None));
        let message = failed.expect_err("the cancel vote is refused");
        let text = message
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(
            text.contains("Transfer: insufficient lamports 0"),
            "{program:?}: {text}"
        );
    }
}

/// A Vector wallet votes to cancel through the `Advance` its key signs; its
/// vote is the last one needed.
#[test]
fn a_vector_member_votes_to_cancel_through_its_advance() {
    for program in BOTH {
        let mut desk = VectorDesk::new();
        let account = desk.add_vector("Quantum", true);
        let squad = {
            let mut chain = desk.chain();
            let voter = funded(&mut chain);
            let members = [
                (voter.pubkey(), PERMISSION_ALL),
                (account.address, PERMISSION_ALL),
            ];
            // 1 of 2: one cancel vote cancels.
            let squad = create(&mut chain, program, &voter, &members, 1, 0, None);
            let payment = solana_system_interface::instruction::transfer(
                &squad.vault(0),
                &KeypairSigner::new_random().pubkey(),
                SOL,
            );
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
            squad
        };
        let read = |desk: &VectorDesk| {
            let (state, entries) =
                run(wallet_flows::squads::load_page(&desk.node, &squad, None)).unwrap();
            let entry = entries.into_iter().find(|entry| entry.index == 1).unwrap();
            (state, entry)
        };
        let (state, entry) = read(&desk);
        assert_eq!(
            entry.proposal.as_ref().unwrap().status,
            ProposalStatus::Approved
        );
        assert!(
            available_cancel(&state, &entry, &account.address)
                .unwrap()
                .last(),
            "{program:?}"
        );
        cancel_through_vector(&mut desk, &squad, &entry, account.address);

        let (state, entry) = read(&desk);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Cancelled, "{program:?}");
        assert_eq!(proposal.cancelled, vec![account.address]);
        assert_eq!(
            available_action(&state, &entry, &account.address, 0),
            Err(NoAction::Cancelled)
        );
    }
}

/// A member added after the proposal was made: the Vector wallet's cancel
/// vote grows the proposal, and its own address pays, inside its
/// `Passthrough`.
#[test]
fn a_vector_member_pays_for_the_growth_from_its_own_address() {
    for program in BOTH {
        let mut desk = VectorDesk::new();
        let account = desk.add_vector("Quantum", true);
        let squad = {
            let mut chain = desk.chain();
            let voter = funded(&mut chain);
            chain.fund(&account.address, SOL / 10);
            let members = [
                (voter.pubkey(), PERMISSION_ALL),
                (account.address, PERMISSION_ALL),
            ];
            let squad = create(
                &mut chain,
                program,
                &voter,
                &members,
                1,
                0,
                Some(voter.pubkey()),
            );
            let payment = solana_system_interface::instruction::transfer(
                &squad.vault(0),
                &KeypairSigner::new_random().pubkey(),
                SOL,
            );
            let by = voter.pubkey();
            let newcomer = KeypairSigner::new_random().pubkey();
            chain
                .send(
                    &[&voter],
                    vec![
                        squad
                            .create_transaction(1, 0, &[payment], &by, &by)
                            .unwrap(),
                        squad.create_proposal(1, &by, &by),
                        squad.approve(1, &by),
                        add_member(&squad, &by, &newcomer, PERMISSION_ALL),
                    ],
                )
                .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
            squad
        };
        let before = desk.chain().balance(&account.address);
        let (state, entries) =
            run(wallet_flows::squads::load_page(&desk.node, &squad, None)).unwrap();
        let entry = entries.into_iter().find(|entry| entry.index == 1).unwrap();
        assert!(entry.stale, "{program:?}");
        assert!(available_cancel(&state, &entry, &account.address).is_some());
        cancel_through_vector(&mut desk, &squad, &entry, account.address);
        let after = desk.chain().balance(&account.address);
        assert!(after < before, "{program:?}: the Vector address paid");
        let (_, entries) = run(wallet_flows::squads::load_page(&desk.node, &squad, None)).unwrap();
        let entry = entries.into_iter().find(|entry| entry.index == 1).unwrap();
        assert_eq!(
            entry.proposal.unwrap().status,
            ProposalStatus::Cancelled,
            "{program:?}"
        );
    }
}

/// A Vector wallet opens a draft through its `Advance`.
#[test]
fn a_vector_member_opens_a_draft_through_its_advance() {
    for program in BOTH {
        let mut desk = VectorDesk::new();
        let account = desk.add_vector("Quantum", true);
        let squad = {
            let mut chain = desk.chain();
            let voter = funded(&mut chain);
            let members = [
                (voter.pubkey(), PERMISSION_ALL),
                (account.address, PERMISSION_INITIATE),
            ];
            let squad = create(&mut chain, program, &voter, &members, 1, 0, None);
            let payment = solana_system_interface::instruction::transfer(
                &squad.vault(0),
                &KeypairSigner::new_random().pubkey(),
                SOL,
            );
            let by = voter.pubkey();
            chain
                .send(
                    &[&voter],
                    vec![
                        squad
                            .create_transaction(1, 0, &[payment], &by, &by)
                            .unwrap(),
                        squad.create_draft(1, &by, &by),
                    ],
                )
                .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
            squad
        };
        let (state, entries) =
            run(wallet_flows::squads::load_page(&desk.node, &squad, None)).unwrap();
        let entry = entries.into_iter().find(|entry| entry.index == 1).unwrap();
        assert_eq!(
            available_action(&state, &entry, &account.address, 0),
            Ok(ProposalAction::Activate)
        );
        let instructions = single(account.address, ProposalAction::Activate)
            .instructions(&squad, &entry)
            .unwrap();
        let (built, _) = run(compile_fresh(&desk.node, desk.payer.pubkey(), instructions)).unwrap();
        let adapted = desk.rebuild_wire(&built);
        let signed = desk
            .sign(&adapted.wire, &adapted.context)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        desk.land(&signed.wire);
        let (_, entries) = run(wallet_flows::squads::load_page(&desk.node, &squad, None)).unwrap();
        let entry = entries.into_iter().find(|entry| entry.index == 1).unwrap();
        assert_eq!(
            entry.proposal.unwrap().status,
            ProposalStatus::Active,
            "{program:?}"
        );
    }
}

fn cancel_through_vector(
    desk: &mut VectorDesk,
    squad: &Squad,
    entry: &wallet_core::proposals::ProposalEntry,
    member: Pubkey,
) {
    let instructions = single(member, ProposalAction::Cancel)
        .instructions(squad, entry)
        .unwrap();
    let (built, _) = run(compile_fresh(&desk.node, desk.payer.pubkey(), instructions)).unwrap();
    let adapted = desk.rebuild_wire(&built);
    let signed = desk
        .sign(&adapted.wire, &adapted.context)
        .unwrap_or_else(|err| panic!("{:?}: {err}", squad.program));
    desk.land(&signed.wire);
}

/// An offline key votes to cancel, alone, on the multisig row's durable
/// nonce: it lands however long the trip to its device took.
#[test]
fn an_offline_member_votes_to_cancel_on_the_rows_durable_nonce() {
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
                &team.fees.pubkey(),
                rent,
            );
            chain.send(&[&team.fees, &nonce_key], create).unwrap();
        }
        team.propose(1, &[team.payment(SOL)]);
        team.approve(1, &voter);

        let (_, entry) = team.modal(1);
        let (moved, height) = run(wallet_flows::squads::compile_request(
            &team.node,
            &team.squad,
            &entry,
            &single(cold.pubkey(), ProposalAction::Cancel),
            team.fees.pubkey(),
            |_| None,
            Some(nonce),
        ))
        .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        assert_eq!(height, None, "{program:?}: on the durable nonce");
        let paid = press_sign(Some(&team.node), &moved, &[&team.fees]);
        team.node.chain().age_blockhashes(200);
        let cold_signed = press_sign(None, &moved, &[&cold]);
        let objects =
            wallet_core::signature_objects(&decode_v1_transaction(&cold_signed.wire).unwrap());
        let QrObject::Signature(back) = carried(&objects[0].signed) else {
            panic!("a signature");
        };
        let (wire, _) = accept_signature(&paid.wire, &back).unwrap();
        team.submit(&wire);
        assert_eq!(team.status(1), ProposalStatus::Cancelled, "{program:?}");
    }
}
