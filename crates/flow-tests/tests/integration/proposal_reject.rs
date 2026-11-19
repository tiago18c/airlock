//! Rejecting a proposal, on the path a person takes, against the real
//! programs: the Proposals modal reads the page and offers Reject beside
//! Approve, the rejection is compiled as it does, Review and Sign simulates
//! and fits it, the keys sign, it is submitted, and the modal, opened again,
//! reads the proposal as the program left it.
//!
//! The cases: a plain member's rejection, the one that reaches the cutoff
//! and rejects for good, a member changing its vote both ways, proposals the
//! app cannot read or whose stored transaction is gone, and members without
//! a key of their own or on another device.

use crate::proposal_execute::{create, funded, press_sign, run, Team};
use crate::support::vector::Desk as VectorDesk;
use crate::support::{carried, logs, Chain, SOL};
use solana_pubkey::Pubkey;
use wallet_core::adapt::plan_adaptation;
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::proposals::{
    available_action, available_rejection, member_choices, NoAction, ProposalAction,
    ProposalRequest, Rejection, Stored,
};
use wallet_core::protocol::QrObject;
use wallet_core::relay::{accept_signature, authority_signature_objects};
use wallet_core::squads::{
    ProposalStatus, SquadsProgram, TableProblem, PERMISSION_ALL, PERMISSION_VOTE,
};
use wallet_core::{decode_v1_transaction, KeypairSigner, WalletRecord, WalletSigner};
use wallet_flows::compile::compile_fresh;

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

fn reject(member: Pubkey) -> ProposalRequest {
    ProposalRequest::Single {
        member,
        action: ProposalAction::Reject,
    }
}

/// Three members that may vote, 2 of 3, so two rejections reject a
/// proposal; and the proposer, which may not vote.
fn three(program: SquadsProgram) -> (Team, [KeypairSigner; 3]) {
    let mut chain = Chain::new();
    let keys = [(); 3].map(|_| funded(&mut chain));
    let members =
        [keys[0].pubkey(), keys[1].pubkey(), keys[2].pubkey()].map(|key| (key, PERMISSION_ALL));
    (Team::on(chain, program, &members, 2, 0, None), keys)
}

impl Team {
    /// `member` rejects `index` as the modal offers it, with the hot wallet
    /// that is no member paying, all the way to the chain.
    fn reject_as(&self, index: u64, member: &KeypairSigner) {
        let (state, entry) = self.modal(index);
        assert!(
            available_rejection(&state, &entry, &member.pubkey()).is_some(),
            "Reject is offered"
        );
        let wire = self.review(&self.build(index, &reject(member.pubkey()), &self.fees.pubkey()));
        let signed = press_sign(Some(&self.node), &wire, &[&self.fees, member]);
        self.submit(&signed.wire);
    }
}

/// A plain member rejects; the next rejection reaches the cutoff and the
/// proposal is rejected for good: the modal offers nothing more, and the
/// program takes neither an approval nor an execution.
#[test]
fn the_rejection_that_reaches_the_cutoff_rejects_for_good() {
    for program in BOTH {
        let (team, [a, b, c]) = three(program);
        team.propose(1, &[team.payment(SOL)]);

        let (state, entry) = team.modal(1);
        let choices = member_choices(&state, &entry, &a.pubkey(), 0);
        assert_eq!(choices.forward, Ok(ProposalAction::Approve), "{program:?}");
        assert_eq!(
            choices.reject,
            Some(Rejection {
                missing_after: 1,
                replaces_approval: false
            })
        );
        // It runs nothing, so it is compiled without extra heap, and the
        // member and the fee payer sign it.
        let built = team.build(1, &reject(a.pubkey()), &team.fees.pubkey());
        let tx = decode_v1_transaction(&built).unwrap();
        assert_eq!(tx.signers.len(), 2, "{program:?}");
        team.reject_as(1, &a);

        let (state, entry) = team.modal(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Active, "{program:?}");
        assert_eq!(proposal.rejected, vec![a.pubkey()]);
        // The one who rejected may still approve, not reject again.
        let mine = member_choices(&state, &entry, &a.pubkey(), 0);
        assert_eq!(mine.forward, Ok(ProposalAction::Approve));
        assert_eq!(mine.reject, None);
        // The next one is the last needed.
        let last = available_rejection(&state, &entry, &b.pubkey()).unwrap();
        assert!(last.last(), "{program:?}");
        team.reject_as(1, &b);

        let (state, entry) = team.modal(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Rejected, "{program:?}");
        for member in [&a, &b, &c] {
            let choices = member_choices(&state, &entry, &member.pubkey(), 0);
            assert_eq!(choices.forward, Err(NoAction::Rejected), "{program:?}");
            assert_eq!(choices.reject, None);
        }
        // Sent anyway, neither an approval nor an execution lands.
        let approve = team.squad.approve(1, &c.pubkey());
        let refused = team.node.chain().send(&[&c], vec![approve]).unwrap_err();
        assert!(
            logs(&refused).contains("InvalidProposalStatus"),
            "{program:?}: {}",
            logs(&refused)
        );
        let execute = team.squad.execute(1, 0, &[team.payment(SOL)], &c.pubkey());
        let refused = team.node.chain().send(&[&c], vec![execute]).unwrap_err();
        assert!(
            logs(&refused).contains("InvalidProposalStatus"),
            "{program:?}: {}",
            logs(&refused)
        );
        assert_eq!(team.node.chain().balance(&team.recipient), 0);
    }
}

/// A member that approved changes to a rejection, which takes its approval
/// back, and then to an approval again, which takes the rejection back. A
/// second rejection by the same member is refused by the program, which is
/// why it is not offered.
#[test]
fn a_member_changes_its_approval_to_a_rejection_and_back() {
    for program in BOTH {
        let (team, [a, b, _]) = three(program);
        team.propose(1, &[team.payment(SOL)]);
        team.approve(1, &a);

        let (state, entry) = team.modal(1);
        let choices = member_choices(&state, &entry, &a.pubkey(), 0);
        assert_eq!(
            choices.forward,
            Err(NoAction::WaitingForOthers { missing: 1 }),
            "{program:?}"
        );
        assert_eq!(
            choices.reject,
            Some(Rejection {
                missing_after: 1,
                replaces_approval: true
            })
        );
        team.reject_as(1, &a);
        let (state, entry) = team.modal(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert!(proposal.approved.is_empty(), "{program:?}");
        assert_eq!(proposal.rejected, vec![a.pubkey()]);

        // And back: Approve, as the modal offers it.
        assert_eq!(
            available_action(&state, &entry, &a.pubkey(), 0),
            Ok(ProposalAction::Approve)
        );
        let approve = ProposalRequest::Single {
            member: a.pubkey(),
            action: ProposalAction::Approve,
        };
        let wire = team.review(&team.build(1, &approve, &team.fees.pubkey()));
        team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees, &a]).wire);
        let (_, entry) = team.modal(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.approved, vec![a.pubkey()], "{program:?}");
        assert!(proposal.rejected.is_empty());
        assert_eq!(proposal.status, ProposalStatus::Active);

        // A rejection cast twice: the program refuses the second.
        team.reject_as(1, &b);
        let again = team.squad.reject(1, &b.pubkey());
        let refused = team.node.chain().send(&[&b], vec![again]).unwrap_err();
        assert!(
            logs(&refused).contains("AlreadyRejected"),
            "{program:?}: {}",
            logs(&refused)
        );
    }
}

/// What the app cannot read is never offered for approval, but it can be
/// rejected: a stored transaction naming lookup table entries that do not
/// exist yet, and one whose stored transaction account is gone. The
/// programs read only the proposal for a rejection.
#[test]
fn what_cannot_be_read_or_is_gone_can_still_be_rejected() {
    for program in BOTH {
        let mut chain = Chain::new();
        let short = chain.plant_lookup_table(&[]);
        let keys = [(); 2].map(|_| funded(&mut chain));
        let members = [keys[0].pubkey(), keys[1].pubkey()].map(|key| (key, PERMISSION_ALL));
        // 2 of 2: one rejection rejects.
        let team = Team::on(chain, program, &members, 2, 0, None);
        let [a, _] = &keys;
        let to = KeypairSigner::new_random().pubkey();
        let payment = solana_system_interface::instruction::transfer(&team.vault, &to, SOL);
        let by = team.proposer.pubkey();
        let store = team
            .squad
            .create_transaction_with_tables(
                1,
                0,
                std::slice::from_ref(&payment),
                &[(short, vec![to])],
                0,
                &by,
                &by,
            )
            .unwrap();
        team.propose_with(1, store);
        team.propose(2, &[team.payment(SOL)]);
        team.node.chain().remove(&team.squad.transaction(2));

        for (index, unread) in [(1, NoAction::Unreadable), (2, NoAction::Gone)] {
            let (state, entry) = team.modal(index);
            match index {
                1 => assert!(
                    matches!(
                        entry.stored,
                        Stored::LookupTables(TableProblem::Short { .. })
                    ),
                    "{program:?}: {:?}",
                    entry.stored
                ),
                _ => assert_eq!(entry.stored, Stored::Missing, "{program:?}"),
            }
            let choices = member_choices(&state, &entry, &a.pubkey(), 0);
            assert_eq!(choices.forward, Err(unread), "{program:?}");
            assert!(choices.reject.unwrap().last());
            team.reject_as(index, a);
            let (state, entry) = team.modal(index);
            assert_eq!(
                entry.proposal.as_ref().unwrap().status,
                ProposalStatus::Rejected,
                "{program:?}"
            );
            assert_eq!(
                available_action(&state, &entry, &a.pubkey(), 0),
                Err(NoAction::Rejected)
            );
        }
    }
}

/// A programmatic signer rejects. Sign rebuilds the rejection around its
/// authorization, as it does an approval, and its authority signs it on
/// another device.
#[test]
fn a_programmatic_member_rejects_through_its_authorization() {
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
        let members = [(voter.pubkey(), PERMISSION_ALL), (member, PERMISSION_VOTE)];
        let team = Team::on(chain, program, &members, 1, 0, None);
        team.propose(1, &[team.payment(SOL)]);

        let wallets = vec![
            WalletRecord::from_keypair_seed("Fees", fees.seed_bytes()),
            signer,
        ];
        let (state, entry) = team.modal(1);
        // Two voters, threshold one: two rejections reject it.
        assert_eq!(
            available_rejection(&state, &entry, &member),
            Some(Rejection {
                missing_after: 1,
                replaces_approval: false
            }),
            "{program:?}"
        );
        let built = team.build(1, &reject(member), &fees.pubkey());
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
        assert_eq!(proposal.rejected, vec![member], "{program:?}");
        assert_eq!(proposal.status, ProposalStatus::Active);
    }
}

/// A Vector wallet rejects. Sign rebuilds the rejection around the
/// `Advance` its key signs, as it does an approval; its rejection is the
/// last one needed here.
#[test]
fn a_vector_member_rejects_through_its_advance() {
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
            // 2 of 2: one rejection rejects.
            let squad = create(&mut chain, program, &voter, &members, 2, 0, None);
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
        assert!(
            available_rejection(&state, &entry, &account.address)
                .unwrap()
                .last(),
            "{program:?}"
        );
        let instructions = reject(account.address)
            .instructions(&squad, &entry)
            .unwrap();
        let (built, _) = run(compile_fresh(&desk.node, desk.payer.pubkey(), instructions)).unwrap();
        let adapted = desk.rebuild_wire(&built);
        let signed = desk
            .sign(&adapted.wire, &adapted.context)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        desk.land(&signed.wire);

        let (state, entry) = read(&desk);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Rejected, "{program:?}");
        assert_eq!(proposal.rejected, vec![account.address]);
        assert_eq!(
            available_action(&state, &entry, &account.address, 0),
            Err(NoAction::Rejected)
        );
    }
}

/// An offline key rejects, alone, on the multisig row's durable nonce, as
/// the modal compiles it for such a member: it lands however long the trip
/// to its device took.
#[test]
fn an_offline_member_rejects_on_the_rows_durable_nonce() {
    for program in BOTH {
        let mut chain = Chain::new();
        let voter = funded(&mut chain);
        let cold = KeypairSigner::new_random();
        let members = [
            (voter.pubkey(), PERMISSION_ALL),
            (cold.pubkey(), PERMISSION_ALL),
        ];
        let team = Team::on(chain, program, &members, 2, 0, None);
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

        let (_, entry) = team.modal(1);
        let (moved, height) = run(wallet_flows::squads::compile_request(
            &team.node,
            &team.squad,
            &entry,
            &reject(cold.pubkey()),
            team.fees.pubkey(),
            |_| None,
            Some(nonce),
        ))
        .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        assert_eq!(height, None, "{program:?}: on the durable nonce");
        assert!(decode_v1_transaction(&moved).unwrap().uses_durable_nonce);
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

        let (_, entry) = team.modal(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Rejected, "{program:?}");
        assert_eq!(proposal.rejected, vec![cold.pubkey()]);
    }
}
