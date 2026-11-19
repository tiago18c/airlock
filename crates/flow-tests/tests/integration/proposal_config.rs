//! Changes to the multisig itself, proposed, read, approved and executed on
//! the path a person takes, against the real programs: the Proposals modal
//! reads the change as its actions, offers Approve and Execute as for a
//! vault transaction, the transaction is compiled as it compiles it, Review
//! and Sign reads what the vote acts on, and after it lands the multisig
//! holds the new members, threshold or time lock.
//!
//! Also what the programs make of it: every earlier proposal goes stale,
//! an approved change that went stale can never be executed, a spending
//! limit's account is created and closed through the execution's remaining
//! accounts, a controlled multisig takes no change by proposal, and members
//! without a key execute with the fee payer paying for the growth.

use crate::proposal_execute::{create, funded, press_sign, run, Team};
use crate::support::vector::Desk as VectorDesk;
use crate::support::{carried, logs, Chain, SOL};
use solana_pubkey::Pubkey;
use wallet_core::adapt::plan_adaptation;
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::proposals::{
    available_action, combined_approval, ChosenMember, MemberKind, NoAction, ProposalAction,
    ProposalRequest, Stored,
};
use wallet_core::protocol::QrObject;
use wallet_core::relay::{accept_signature, authority_signature_objects};
use wallet_core::squads::{
    change_lines, ConfigAction, Period, ProposalStatus, SpendingLimit, SquadsMember, SquadsProgram,
    PERMISSION_ALL, PERMISSION_EXECUTE, PERMISSION_VOTE,
};
use wallet_core::{
    blind_in, decode_v1_transaction, explain_transaction, squad_state_in, stored_content_in,
    Instruction, KeypairSigner, NoIdls, StoredContent, WalletId, WalletRecord, WalletSigner,
};
use wallet_flows::compile::compile_fresh;

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

fn single(member: Pubkey, action: ProposalAction) -> ProposalRequest {
    ProposalRequest::Single { member, action }
}

/// Three members that may do everything, 2 of 3, and the proposer, which
/// may only propose.
fn three(program: SquadsProgram) -> (Team, [KeypairSigner; 3]) {
    let mut chain = Chain::new();
    let keys = [(); 3].map(|_| funded(&mut chain));
    let members =
        [keys[0].pubkey(), keys[1].pubkey(), keys[2].pubkey()].map(|key| (key, PERMISSION_ALL));
    (Team::on(chain, program, &members, 2, 0, None), keys)
}

impl Team {
    /// The proposer stores `actions` as change `index` and opens voting on
    /// it.
    fn propose_change(&self, index: u64, actions: &[ConfigAction]) {
        let by = self.proposer.pubkey();
        let store = self
            .squad
            .create_change(index, actions, &by, &by)
            .expect("the change is stored");
        self.propose_with(index, store);
    }

    /// `member` does `action` to `index` as the modal offers it, the hot
    /// wallet that is no member paying, all the way to the chain.
    fn act_as(&self, index: u64, member: &KeypairSigner, action: ProposalAction) {
        let built = self.build(index, &single(member.pubkey(), action), &self.fees.pubkey());
        let wire = self.review(&built);
        let signed = press_sign(Some(&self.node), &wire, &[&self.fees, member]);
        self.submit(&signed.wire);
    }

    /// What the multisig holds on chain now.
    fn state_now(&self) -> wallet_core::squads::SquadState {
        let data = self.node.chain().data(&self.squad.address);
        self.squad.parse_state(&data).unwrap()
    }
}

fn keys_of(state: &wallet_core::squads::SquadState) -> Vec<Pubkey> {
    state.members.iter().map(|member| member.key).collect()
}

/// A member proposes adding a member and raising the threshold; another
/// reads it as the modal does and approves; a third approves and executes
/// it in one transaction. The multisig then holds the new member and
/// threshold, and the proposal opened before it is stale: the programs
/// refuse a vote on it.
#[test]
fn a_member_is_added_and_the_threshold_raised_by_proposal() {
    for program in BOTH {
        let (team, [a, b, _c]) = three(program);
        // An ordinary proposal, open, made before the change.
        team.propose(1, &[team.payment(SOL / 4)]);
        team.approve(1, &b);

        let newcomer = KeypairSigner::new_random().pubkey();
        let actions = [
            ConfigAction::AddMember(SquadsMember {
                key: newcomer,
                permissions: PERMISSION_VOTE,
            }),
            ConfigAction::ChangeThreshold(3),
        ];
        team.propose_change(2, &actions);

        // The modal reads the change, in words.
        let (state, entry) = team.modal(2);
        let Stored::Config(read) = &entry.stored else {
            panic!("{program:?}: {:?}", entry.stored);
        };
        assert_eq!(read.actions, actions, "{program:?}");
        assert_eq!(read.creator, team.proposer.pubkey());
        let name = |key: &Pubkey| {
            if *key == newcomer {
                "Erin".to_string()
            } else {
                key.to_string()
            }
        };
        let said: Vec<String> = change_lines(Some(&state), &read.actions)
            .iter()
            .map(|line| line.text(&name))
            .collect();
        assert_eq!(
            said,
            [
                "Add Erin as a member that may vote",
                "Change the threshold from 2 to 3 out of 4 voting members"
            ]
        );
        assert_eq!(
            available_action(&state, &entry, &b.pubkey(), 0),
            Ok(ProposalAction::Approve),
            "{program:?}"
        );
        team.act_as(2, &b, ProposalAction::Approve);

        // Review and Sign reads what the last approval acts on: the change,
        // and the multisig it starts from.
        let (state, entry) = team.modal(2);
        assert_eq!(
            available_action(&state, &entry, &a.pubkey(), 0),
            Ok(ProposalAction::ApproveAndExecute),
            "{program:?}"
        );
        let built = team.build(
            2,
            &single(a.pubkey(), ProposalAction::ApproveAndExecute),
            &team.fees.pubkey(),
        );
        let tx = decode_v1_transaction(&built).unwrap();
        let blind = blind_in(&explain_transaction(&NoIdls, &tx));
        assert_eq!(blind.len(), 2, "{program:?}: the approval and the run");
        for approval in &blind {
            let target = approval.target.expect("what it acts on can be read");
            let behind = run(wallet_flows::squads::read_stored(&team.node, &[target])).unwrap();
            assert_eq!(
                stored_content_in(&target, &behind),
                Some(StoredContent::Change(read.clone())),
                "{program:?}"
            );
            assert_eq!(squad_state_in(&target, &behind), Some(state.clone()));
        }

        let wire = team.review(&built);
        team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees, &a]).wire);

        let after = team.state_now();
        assert_eq!(after.threshold, 3, "{program:?}");
        assert!(keys_of(&after).contains(&newcomer));
        assert_eq!(after.members.len(), 5);
        let (_, executed) = team.modal(2);
        assert_eq!(
            executed.proposal.unwrap().status,
            ProposalStatus::Executed,
            "{program:?}"
        );

        // The proposal opened before the change is stale now.
        let (state, old) = team.modal(1);
        assert!(old.stale, "{program:?}");
        assert_eq!(
            available_action(&state, &old, &a.pubkey(), 0),
            Err(NoAction::Stale)
        );
        let refused = team
            .node
            .chain()
            .send(&[&a], vec![team.squad.approve(1, &a.pubkey())])
            .unwrap_err();
        assert!(
            logs(&refused).contains("StaleProposal"),
            "{program:?}: {}",
            logs(&refused)
        );
    }
}

/// A member removed and the time lock set, voted by two, executed by one
/// of them in a transaction of its own.
#[test]
fn a_member_is_removed_and_a_time_lock_set_by_proposal() {
    for program in BOTH {
        let (team, [a, b, c]) = three(program);
        let actions = [
            ConfigAction::RemoveMember(c.pubkey()),
            ConfigAction::SetTimeLock(60),
        ];
        team.propose_change(1, &actions);
        team.act_as(1, &a, ProposalAction::Approve);
        team.act_as(1, &b, ProposalAction::Approve);
        let (state, entry) = team.modal(1);
        assert_eq!(
            entry.proposal.as_ref().unwrap().status,
            ProposalStatus::Approved
        );
        assert_eq!(
            available_action(&state, &entry, &a.pubkey(), 0),
            Ok(ProposalAction::Execute),
            "{program:?}"
        );
        team.act_as(1, &a, ProposalAction::Execute);
        let after = team.state_now();
        assert!(!keys_of(&after).contains(&c.pubkey()), "{program:?}");
        assert_eq!((after.threshold, after.time_lock), (2, 60));
    }
}

/// Two changes approved together: once one is executed, the other is stale,
/// and both programs refuse to execute it, though it is approved. The
/// modal says so and offers nothing but the vote to cancel.
#[test]
fn an_approved_change_that_went_stale_can_never_be_executed() {
    for program in BOTH {
        let (team, [a, b, _c]) = three(program);
        team.propose_change(1, &[ConfigAction::SetTimeLock(30)]);
        team.propose_change(2, &[ConfigAction::SetTimeLock(90)]);
        for index in [1, 2] {
            team.approve(index, &a);
            team.approve(index, &b);
        }
        team.act_as(1, &a, ProposalAction::Execute);
        assert_eq!(team.state_now().time_lock, 30);

        let (state, entry) = team.modal(2);
        assert!(entry.stale, "{program:?}");
        assert_eq!(
            entry.proposal.as_ref().unwrap().status,
            ProposalStatus::Approved
        );
        assert_eq!(
            available_action(&state, &entry, &a.pubkey(), i64::MAX),
            Err(NoAction::StaleChange),
            "{program:?}"
        );
        let Stored::Config(change) = &entry.stored else {
            panic!("a change");
        };
        let execute = team
            .squad
            .execute_change(change, &a.pubkey(), &a.pubkey())
            .unwrap();
        // Past the new time lock, which the programs check first.
        team.node.chain().pass_seconds(31);
        let refused = team.node.chain().send(&[&a], vec![execute]).unwrap_err();
        assert!(
            logs(&refused).contains("StaleProposal"),
            "{program:?}: {}",
            logs(&refused)
        );
        // It can still be cancelled.
        assert!(wallet_core::proposals::available_cancel(&state, &entry, &a.pubkey()).is_some());
        team.act_as(2, &a, ProposalAction::Cancel);
    }
}

/// A spending limit added and removed by proposal: the execution names its
/// account after its own, the program creates it with the fee payer paying,
/// and closes it to the fee payer again.
#[test]
fn a_spending_limit_is_added_and_removed_by_proposal() {
    for program in BOTH {
        let (team, [a, b, _c]) = three(program);
        let seed = KeypairSigner::new_random().pubkey();
        let limit = SpendingLimit {
            seed,
            vault_index: 0,
            mint: Pubkey::default(),
            amount: SOL,
            period: Period::Day,
            members: vec![a.pubkey()],
            destinations: Vec::new(),
            expiration: match program {
                SquadsProgram::V4 => None,
                SquadsProgram::SmartAccount => Some(i64::MAX),
            },
        };
        team.propose_change(1, &[ConfigAction::AddSpendingLimit(limit.clone())]);
        let (_, entry) = team.modal(1);
        let Stored::Config(read) = &entry.stored else {
            panic!("{program:?}: {:?}", entry.stored);
        };
        assert_eq!(read.actions, [ConfigAction::AddSpendingLimit(limit)]);
        team.approve(1, &b);
        team.act_as(1, &a, ProposalAction::ApproveAndExecute);
        let account = team.squad.spending_limit(&seed);
        let (owner, rent) = {
            let chain = team.node.chain();
            (
                chain.account(&account).map(|found| found.owner),
                chain.balance(&account),
            )
        };
        assert_eq!(owner, Some(program.program_id()), "{program:?}");
        assert!(rent > 0);

        // Adding a spending limit leaves the members alone: nothing went
        // stale, so the next change is voted on as usual.
        team.propose_change(2, &[ConfigAction::RemoveSpendingLimit(account)]);
        team.approve(2, &b);
        let fees_before = team.node.chain().balance(&team.fees.pubkey());
        team.act_as(2, &a, ProposalAction::ApproveAndExecute);
        let chain = team.node.chain();
        assert_eq!(chain.balance(&account), 0, "{program:?}: closed");
        // The fee payer paid one fee and got the rent back.
        assert!(chain.balance(&team.fees.pubkey()) > fees_before + rent - SOL / 100);
    }
}

/// A multisig with a config or settings authority takes no change by
/// proposal: both programs refuse to store one (`NotSupportedForControlled`).
#[test]
fn a_controlled_multisig_takes_no_change_by_proposal() {
    for program in BOTH {
        let mut chain = Chain::new();
        let [a, b, authority] = [(); 3].map(|_| funded(&mut chain));
        let members = [a.pubkey(), b.pubkey()].map(|key| (key, PERMISSION_ALL));
        let team = Team::on(chain, program, &members, 2, 0, Some(authority.pubkey()));
        let by = team.proposer.pubkey();
        let store = team
            .squad
            .create_change(1, &[ConfigAction::ChangeThreshold(1)], &by, &by)
            .unwrap();
        let refused = team
            .node
            .chain()
            .send(&[&team.proposer], vec![store])
            .unwrap_err();
        assert!(
            logs(&refused).contains("NotSupportedForControlled"),
            "{program:?}: {}",
            logs(&refused)
        );
    }
}

/// A programmatic signer executes a change that grows the multisig. Sign
/// rebuilds the execution around its authorization; the fee payer, which
/// signs the transaction anyway, pays for the growth, so the member's own
/// address needs nothing.
#[test]
fn a_programmatic_member_executes_a_change_with_the_fee_payer_paying() {
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
        assert_eq!(team.node.chain().balance(&member), 0);
        let newcomer = KeypairSigner::new_random().pubkey();
        team.propose_change(
            1,
            &[ConfigAction::AddMember(SquadsMember {
                key: newcomer,
                permissions: PERMISSION_VOTE,
            })],
        );
        team.approve(1, &voter);

        let wallets = vec![
            WalletRecord::from_keypair_seed("Fees", fees.seed_bytes()),
            signer,
        ];
        let (state, entry) = team.modal(1);
        assert_eq!(
            available_action(&state, &entry, &member, 0),
            Ok(ProposalAction::Execute),
            "{program:?}"
        );
        let built = team.build(1, &single(member, ProposalAction::Execute), &fees.pubkey());
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
        assert!(
            keys_of(&team.state_now()).contains(&newcomer),
            "{program:?}"
        );
        assert_eq!(team.node.chain().balance(&member), 0);
    }
}

/// A Vector wallet executes a change that grows the multisig, through the
/// `Advance` its key signs, with the fee payer paying for the growth.
#[test]
fn a_vector_member_executes_a_change_with_the_fee_payer_paying() {
    for program in BOTH {
        let mut desk = VectorDesk::new();
        let account = desk.add_vector("Quantum", true);
        let newcomer = KeypairSigner::new_random().pubkey();
        let squad = {
            let mut chain = desk.chain();
            let voter = funded(&mut chain);
            let members = [
                (voter.pubkey(), PERMISSION_ALL),
                (account.address, PERMISSION_EXECUTE),
            ];
            let squad = create(&mut chain, program, &voter, &members, 1, 0, None);
            let by = voter.pubkey();
            let add = ConfigAction::AddMember(SquadsMember {
                key: newcomer,
                permissions: PERMISSION_VOTE,
            });
            let store: Instruction = squad.create_change(1, &[add], &by, &by).unwrap();
            chain
                .send(
                    &[&voter],
                    vec![
                        store,
                        squad.create_proposal(1, &by, &by),
                        squad.approve(1, &by),
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
            Ok(ProposalAction::Execute),
            "{program:?}"
        );
        let payer = desk.payer.pubkey();
        let instructions = single(account.address, ProposalAction::Execute)
            .instructions_paid_by(&squad, &entry, &payer)
            .unwrap();
        let (built, _) = run(compile_fresh(&desk.node, payer, instructions)).unwrap();
        let adapted = desk.rebuild_wire(&built);
        let signed = desk
            .sign(&adapted.wire, &adapted.context)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        desk.land(&signed.wire);
        let data = desk.chain().data(&squad.address);
        let after = squad.parse_state(&data).unwrap();
        assert!(keys_of(&after).contains(&newcomer), "{program:?}");
    }
}

/// Two members of this device cast both approvals a change needs and
/// execute it in one transaction, as "Approve as 2 members and execute"
/// does for a vault transaction.
#[test]
fn two_members_approve_and_execute_a_change_in_one_transaction() {
    for program in BOTH {
        let (team, [a, b, _c]) = three(program);
        let newcomer = KeypairSigner::new_random().pubkey();
        team.propose_change(
            1,
            &[ConfigAction::AddMember(SquadsMember {
                key: newcomer,
                permissions: PERMISSION_ALL,
            })],
        );
        let (state, entry) = team.modal(1);
        let chosen: Vec<ChosenMember> = [&a, &b]
            .iter()
            .map(|key| ChosenMember {
                wallet: WalletId::new(),
                address: key.pubkey(),
                permissions: PERMISSION_ALL,
                kind: MemberKind::HotKey,
            })
            .collect();
        let combined = combined_approval(&state, &entry, &chosen, None)
            .unwrap_or_else(|| panic!("{program:?}: both can approve and execute"));
        assert_eq!(combined.voters, [a.pubkey(), b.pubkey()]);
        let built = team.build(1, &ProposalRequest::Combined(combined), &team.fees.pubkey());
        let wire = team.review(&built);
        team.submit(&press_sign(Some(&team.node), &wire, &[&team.fees, &a, &b]).wire);
        let after = team.state_now();
        assert!(keys_of(&after).contains(&newcomer), "{program:?}");
    }
}
