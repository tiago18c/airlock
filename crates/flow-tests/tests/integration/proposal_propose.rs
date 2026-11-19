//! Proposing a change to the multisig itself from a wallet's Settings, on
//! the path a person takes, against the real programs: the multisig is read,
//! the change vetted, the shape decided from the members of this device,
//! the transaction compiled as the app compiles it, Review and Sign reads it
//! (nothing in it is blind: it holds what it approves), it is signed and
//! submitted, and the multisig then holds the new members, threshold or rent
//! collector, or the proposal waits in the Proposals modal for votes.
//!
//! Also: the rent collector set this way lets a finished transaction's
//! accounts be closed (v4), a programmatic signer and a Vector wallet propose
//! through their authorizations, a member that may not propose is refused
//! before anything is built (and the programs refuse it too), and a
//! controlled multisig takes no proposal.

use crate::proposal_execute::{funded, press_sign, run, Team};
use crate::support::vector::Desk as VectorDesk;
use crate::support::{carried, logs, Chain};
use solana_pubkey::Pubkey;
use wallet_core::adapt::plan_adaptation;
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::proposals::{
    available_action, available_close, change_proposal, closable, vet_change, CannotPropose,
    ChangeDraft, ChangeProposal, ChosenMember, MemberKind, ProposalAction, ProposalRequest, Stored,
};
use wallet_core::protocol::QrObject;
use wallet_core::relay::{accept_signature, authority_signature_objects};
use wallet_core::squads::{
    ConfigAction, ProposalStatus, Squad, SquadState, SquadsMember, SquadsProgram, PERMISSION_ALL,
    PERMISSION_EXECUTE, PERMISSION_INITIATE, PERMISSION_VOTE,
};
use wallet_core::{
    blind_in, decode_v1_transaction, explain_transaction, KeypairSigner, NoIdls, WalletId,
    WalletRecord, WalletSigner,
};
use wallet_flows::squads::{look_up, Lookup};

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

fn hot(key: &KeypairSigner) -> ChosenMember {
    ChosenMember {
        wallet: WalletId::new(),
        address: key.pubkey(),
        permissions: PERMISSION_ALL,
        kind: MemberKind::HotKey,
    }
}

fn read(node: &crate::support::node::Node, squad: &Squad) -> SquadState {
    match run(look_up(Some(node), squad)) {
        Lookup::Found(state) => state,
        other => panic!("{other:?}"),
    }
}

fn keys_of(state: &SquadState) -> Vec<Pubkey> {
    state.members.iter().map(|member| member.key).collect()
}

/// What Propose does, up to the transaction opened on Review and Sign:
/// read the multisig, vet `draft`, decide the shape from `chosen`, compile
/// with the team's fee payer paying. Review and Sign finds nothing blind in
/// it, and simulates and fits it.
fn propose(team: &Team, draft: &ChangeDraft, chosen: &[ChosenMember]) -> (Vec<u8>, ChangeProposal) {
    let program = team.squad.program;
    let state = read(&team.node, &team.squad);
    let actions = draft.actions(&state);
    let now = team.node.chain().now();
    vet_change(program, &state, &actions, now).unwrap_or_else(|why| panic!("{program:?}: {why:?}"));
    let proposal = change_proposal(&state, chosen, None).unwrap();
    assert_eq!(proposal.index, state.transaction_index + 1);
    let (wire, _) = run(wallet_flows::squads::propose_change(
        &team.node,
        &team.squad,
        &actions,
        &proposal,
        team.fees.pubkey(),
        |_| None,
        None,
    ))
    .unwrap_or_else(|err| panic!("{program:?}: {err}"));
    let tx = decode_v1_transaction(&wire).unwrap();
    assert!(
        blind_in(&explain_transaction(&NoIdls, &tx)).is_empty(),
        "{program:?}: it holds what it approves"
    );
    (team.review(&wire), proposal)
}

/// Three members that may do everything, 2 of 3 (and the team's proposer,
/// which may only propose).
fn three(program: SquadsProgram) -> (Team, [KeypairSigner; 3]) {
    let mut chain = Chain::new();
    let keys = [(); 3].map(|_| funded(&mut chain));
    let members =
        [keys[0].pubkey(), keys[1].pubkey(), keys[2].pubkey()].map(|key| (key, PERMISSION_ALL));
    (Team::on(chain, program, &members, 2, 0, None), keys)
}

fn add(key: Pubkey, permissions: u8) -> Option<SquadsMember> {
    Some(SquadsMember { key, permissions })
}

/// From a 2 of 3, one member proposes adding a member and raising the
/// threshold in one change: stored, opened and approved by it. A second
/// member approves and executes it from the Proposals modal, and the
/// multisig holds the new member and threshold.
#[test]
fn a_member_is_added_and_the_threshold_raised_in_one_proposal() {
    for program in BOTH {
        let (team, [a, b, _c]) = three(program);
        let newcomer = KeypairSigner::new_random().pubkey();
        let draft = ChangeDraft {
            add: add(newcomer, PERMISSION_ALL),
            threshold: Some(3),
            ..ChangeDraft::default()
        };
        let (wire, proposal) = propose(&team, &draft, &[hot(&a)]);
        assert_eq!(
            (proposal.voters.clone(), proposal.executor),
            (vec![a.pubkey()], None)
        );
        team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees, &a]).wire);

        // It waits in the Proposals modal, read as the change it is.
        let (state, entry) = team.modal(1);
        let Stored::Config(stored) = &entry.stored else {
            panic!("{program:?}: {:?}", entry.stored);
        };
        assert_eq!(
            stored.actions,
            [
                ConfigAction::AddMember(SquadsMember {
                    key: newcomer,
                    permissions: PERMISSION_ALL
                }),
                ConfigAction::ChangeThreshold(3)
            ]
        );
        assert_eq!(stored.creator, a.pubkey());
        assert_eq!(entry.proposal.as_ref().unwrap().approved, vec![a.pubkey()]);
        assert_eq!(
            available_action(&state, &entry, &b.pubkey(), 0),
            Ok(ProposalAction::ApproveAndExecute),
            "{program:?}"
        );
        let request = ProposalRequest::Single {
            member: b.pubkey(),
            action: ProposalAction::ApproveAndExecute,
        };
        let built = team.review(&team.build(1, &request, &team.fees.pubkey()));
        team.submit(&press_sign(Some(&team.node), &built, &[&team.fees, &b]).wire);

        let after = read(&team.node, &team.squad);
        assert_eq!(after.threshold, 3, "{program:?}");
        assert!(keys_of(&after).contains(&newcomer));
        team.assert_executed(1, &b.pubkey());
    }
}

/// A 1 of 1: the member proposes, approves and executes in one
/// transaction, and the change holds at once.
#[test]
fn a_lone_member_proposes_approves_and_executes_in_one_transaction() {
    for program in BOTH {
        let mut chain = Chain::new();
        let a = funded(&mut chain);
        let team = Team::on(chain, program, &[(a.pubkey(), PERMISSION_ALL)], 1, 0, None);
        let newcomer = KeypairSigner::new_random().pubkey();
        let draft = ChangeDraft {
            add: add(newcomer, PERMISSION_VOTE),
            threshold: Some(2),
            ..ChangeDraft::default()
        };
        let (wire, proposal) = propose(&team, &draft, &[hot(&a)]);
        assert_eq!(
            (proposal.voters.clone(), proposal.executor),
            (vec![a.pubkey()], Some(a.pubkey())),
            "{program:?}"
        );
        team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees, &a]).wire);
        let after = read(&team.node, &team.squad);
        assert_eq!(after.threshold, 2, "{program:?}");
        assert!(keys_of(&after).contains(&newcomer));
        let (_, entry) = team.modal(1);
        assert_eq!(entry.proposal.unwrap().status, ProposalStatus::Executed);
    }
}

/// Two members of this device remove the third in one transaction: the
/// acting member stores and approves, the one set to approve together with
/// it approves, and the change is executed.
#[test]
fn a_member_is_removed_by_the_members_that_approve_together() {
    for program in BOTH {
        let (team, [a, b, c]) = three(program);
        let draft = ChangeDraft {
            remove: Some(c.pubkey()),
            ..ChangeDraft::default()
        };
        let (wire, proposal) = propose(&team, &draft, &[hot(&a), hot(&b)]);
        assert_eq!(proposal.executor, Some(a.pubkey()), "{program:?}");
        team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees, &a, &b]).wire);
        let after = read(&team.node, &team.squad);
        assert!(!keys_of(&after).contains(&c.pubkey()), "{program:?}");
        assert_eq!(after.threshold, 2);
    }
}

/// v4: a multisig made without a rent collector gets one by proposal, and
/// then a finished transaction's accounts close, the rent arriving at the
/// collector. The change's own accounts are the first to go.
#[test]
fn a_rent_collector_set_by_proposal_lets_finished_transactions_close() {
    let program = SquadsProgram::V4;
    let (team, [a, b, _c]) = three(program);
    let collector = KeypairSigner::new_random().pubkey();
    let (state, entries) = run(wallet_flows::squads::load_page(
        &team.node,
        &team.squad,
        None,
    ))
    .unwrap();
    assert_eq!((state.rent_collector, entries.len()), (None, 0));
    let draft = ChangeDraft {
        rent_collector: Some(Some(collector)),
        ..ChangeDraft::default()
    };
    let (wire, proposal) = propose(&team, &draft, &[hot(&a), hot(&b)]);
    assert_eq!(proposal.executor, Some(a.pubkey()));
    team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees, &a, &b]).wire);
    assert_eq!(
        read(&team.node, &team.squad).rent_collector,
        Some(collector)
    );

    // The executed change closes now, its rent going to the collector.
    let (state, entries) = run(wallet_flows::squads::load_page(
        &team.node,
        &team.squad,
        None,
    ))
    .unwrap();
    let entry = entries.iter().find(|entry| entry.index == 1).unwrap();
    let closing = available_close(program, &state, entry).unwrap_or_else(|why| panic!("{why:?}"));
    let rent = closing.rent();
    assert!(rent > 0);
    let closings = closable(program, &state, &entries);
    let request = ProposalRequest::Close(closings);
    let (wire, _) = run(wallet_flows::squads::compile_request(
        &team.node,
        &team.squad,
        entry,
        &request,
        team.fees.pubkey(),
        |_| None,
        None,
    ))
    .unwrap();
    let before = team.node.chain().balance(&collector);
    let wire = team.review(&wire);
    team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees]).wire);
    assert_eq!(team.node.chain().balance(&collector), before + rent);
    assert!(team
        .node
        .chain()
        .account(&team.squad.transaction(1))
        .is_none());
}

/// The smart account program has no rent collector: the form has no such
/// field there, and a change that asks for one is not proposable.
#[test]
fn the_smart_account_program_takes_no_rent_collector() {
    let (team, _) = three(SquadsProgram::SmartAccount);
    let state = read(&team.node, &team.squad);
    let draft = ChangeDraft {
        rent_collector: Some(Some(KeypairSigner::new_random().pubkey())),
        ..ChangeDraft::default()
    };
    assert_eq!(
        vet_change(team.squad.program, &state, &draft.actions(&state), 0),
        Err(CannotPropose::NotOnThisProgram)
    );
}

/// A programmatic signer that is the only voter proposes, approves and
/// executes a change in one transaction, through one authorization its
/// authority signs on another device; the fee payer pays the rent and the
/// growth, so the member's own address holds nothing.
#[test]
fn a_programmatic_signer_proposes_through_its_authorization() {
    for program in BOTH {
        let mut chain = Chain::new();
        let fees = funded(&mut chain);
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
        let mut team = Team::on(chain, program, &[(member, PERMISSION_ALL)], 1, 0, None);
        team.fees = fees;
        let newcomer = KeypairSigner::new_random().pubkey();
        let draft = ChangeDraft {
            add: add(newcomer, PERMISSION_VOTE),
            ..ChangeDraft::default()
        };
        let chosen = [ChosenMember {
            wallet: signer.id,
            address: member,
            permissions: PERMISSION_ALL,
            kind: MemberKind::Programmatic { has_nonce: true },
        }];
        let state = read(&team.node, &team.squad);
        let actions = draft.actions(&state);
        let proposal = change_proposal(&state, &chosen, None).unwrap();
        assert_eq!(proposal.executor, Some(member), "{program:?}");
        let (wire, _) = run(wallet_flows::squads::propose_change(
            &team.node,
            &team.squad,
            &actions,
            &proposal,
            team.fees.pubkey(),
            |_| None,
            None,
        ))
        .unwrap();
        // Review and Sign rebuilds it around the authorization.
        let wallets = vec![
            WalletRecord::from_keypair_seed("Fees", team.fees.seed_bytes()),
            signer,
        ];
        let tx = decode_v1_transaction(&wire).unwrap();
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
        let done = press_sign(Some(&team.node), &authorized, &[&team.fees]);
        team.submit(&done.wire);
        assert!(
            keys_of(&read(&team.node, &team.squad)).contains(&newcomer),
            "{program:?}"
        );
        assert_eq!(team.node.chain().balance(&member), 0);
    }
}

/// A Vector wallet that is the only voter proposes, approves and executes a
/// change in one transaction, through the `Advance` its key signs.
#[test]
fn a_vector_wallet_proposes_through_its_advance() {
    for program in BOTH {
        let mut desk = VectorDesk::new();
        let account = desk.add_vector("Quantum", true);
        let newcomer = KeypairSigner::new_random().pubkey();
        let squad = {
            let mut chain = desk.chain();
            let creator = funded(&mut chain);
            crate::proposal_execute::create(
                &mut chain,
                program,
                &creator,
                &[(account.address, PERMISSION_ALL)],
                1,
                0,
                None,
            )
        };
        let state = read(&desk.node, &squad);
        let draft = ChangeDraft {
            add: add(newcomer, PERMISSION_VOTE),
            ..ChangeDraft::default()
        };
        let actions = draft.actions(&state);
        let chosen = [ChosenMember {
            wallet: account.wallet,
            address: account.address,
            permissions: PERMISSION_ALL,
            kind: MemberKind::Vector,
        }];
        let proposal = change_proposal(&state, &chosen, None).unwrap();
        assert_eq!(proposal.executor, Some(account.address));
        let (wire, _) = run(wallet_flows::squads::propose_change(
            &desk.node,
            &squad,
            &actions,
            &proposal,
            desk.payer.pubkey(),
            |_| None,
            None,
        ))
        .unwrap();
        let adapted = desk.rebuild_wire(&wire);
        let signed = desk
            .sign(&adapted.wire, &adapted.context)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        desk.land(&signed.wire);
        let data = desk.chain().data(&squad.address);
        let after = squad.parse_state(&data).unwrap();
        assert!(keys_of(&after).contains(&newcomer), "{program:?}");
    }
}

/// A member that may vote and execute but not propose is refused before
/// anything is built, in one sentence; the programs refuse its change too.
#[test]
fn a_member_that_may_not_propose_is_refused_before_anything_is_built() {
    for program in BOTH {
        let mut chain = Chain::new();
        let [a, b] = [(); 2].map(|_| funded(&mut chain));
        let members = [
            (a.pubkey(), PERMISSION_VOTE | PERMISSION_EXECUTE),
            (b.pubkey(), PERMISSION_ALL),
        ];
        let team = Team::on(chain, program, &members, 1, 0, None);
        let state = read(&team.node, &team.squad);
        let mut chosen = hot(&a);
        chosen.permissions = PERMISSION_VOTE | PERMISSION_EXECUTE;
        let refused = change_proposal(&state, &[chosen], None).unwrap_err();
        assert_eq!(refused, CannotPropose::MayNotPropose(a.pubkey()));
        assert_eq!(
            refused.said().text(&|_| "Alice".to_string()),
            "Alice may not propose."
        );
        // What the app does not build, the program would not take.
        let store = team
            .squad
            .create_change(
                1,
                &[ConfigAction::ChangeThreshold(2)],
                &a.pubkey(),
                &a.pubkey(),
            )
            .unwrap();
        let failure = team.node.chain().send(&[&a], vec![store]).unwrap_err();
        assert!(
            logs(&failure).contains("Unauthorized"),
            "{program:?}: {}",
            logs(&failure)
        );
    }
}

/// A controlled multisig takes no change by proposal: the form is not
/// offered, and the programs refuse to store one.
#[test]
fn a_controlled_multisig_is_not_offered_a_proposal() {
    for program in BOTH {
        let mut chain = Chain::new();
        let [a, authority] = [(); 2].map(|_| funded(&mut chain));
        let team = Team::on(
            chain,
            program,
            &[(a.pubkey(), PERMISSION_ALL)],
            1,
            0,
            Some(authority.pubkey()),
        );
        let state = read(&team.node, &team.squad);
        let draft = ChangeDraft {
            threshold: Some(2),
            add: add(KeypairSigner::new_random().pubkey(), PERMISSION_ALL),
            ..ChangeDraft::default()
        };
        assert_eq!(
            vet_change(program, &state, &draft.actions(&state), 0),
            Err(CannotPropose::Controlled)
        );
        assert_eq!(
            change_proposal(&state, &[hot(&a)], None),
            Err(CannotPropose::Controlled)
        );
        let store = team
            .squad
            .create_change(1, &draft.actions(&state), &a.pubkey(), &a.pubkey())
            .unwrap();
        let failure = team.node.chain().send(&[&a], vec![store]).unwrap_err();
        assert!(
            logs(&failure).contains("NotSupportedForControlled"),
            "{program:?}: {}",
            logs(&failure)
        );
    }
}

/// A member that only proposes stores and opens a change and leaves the
/// votes to the others; a proposal that lands first takes the number, and
/// the one built before it then fails on chain.
#[test]
fn a_proposal_built_on_a_number_taken_since_fails() {
    for program in BOTH {
        let (team, [a, _b, _c]) = three(program);
        let mut proposer = hot(&team.proposer);
        proposer.permissions = PERMISSION_INITIATE;
        let draft = ChangeDraft {
            threshold: Some(1),
            ..ChangeDraft::default()
        };
        let (late, proposal) = propose(&team, &draft, &[proposer]);
        assert_eq!((proposal.voters.len(), proposal.executor), (0, None));
        // Another proposal takes number 1 first.
        team.propose(1, &[team.payment(1_000)]);
        let signed = press_sign(Some(&team.node), &late, &[&team.fees, &team.proposer]);
        let failed = run(wallet_flows::send::submit(&team.node, &signed.wire));
        assert!(failed.is_err(), "{program:?}");
        // Built again, it takes the next number.
        let (wire, again) = propose(&team, &draft, &[hot(&a)]);
        assert_eq!(again.index, 2);
        team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees, &a]).wire);
        let (_, entry) = team.modal(2);
        assert!(matches!(entry.stored, Stored::Config(_)), "{program:?}");
    }
}
