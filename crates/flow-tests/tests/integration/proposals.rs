//! Browsing a multisig's proposals and acting on them, against the real
//! programs.
//!
//! One member stores a transaction and opens its proposal. Another reads the
//! multisig the way the Proposals modal does, from accounts alone, and
//! approves and executes what it finds there.

use crate::support::{logs, Chain, SOL};
use solana_pubkey::Pubkey;
use wallet_core::proposals::{
    action_instructions, available_action, page_accounts, page_indexes, read_page, NoAction,
    ProposalAction, ProposalEntry, Stored,
};
use wallet_core::squads::{
    ProposalStatus, Squad, SquadState, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL,
};
use wallet_core::{Instruction, KeypairSigner, WalletSigner};

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

struct Desk {
    chain: Chain,
    squad: Squad,
    vault: Pubkey,
    a: KeypairSigner,
    b: KeypairSigner,
    recipient: Pubkey,
}

impl Desk {
    /// A multisig of `a` and `b` that needs both, with three SOL in its vault.
    fn new(program: SquadsProgram) -> Self {
        let mut chain = Chain::new();
        let (a, b) = (KeypairSigner::new_random(), KeypairSigner::new_random());
        chain.fund(&a.pubkey(), 10 * SOL);
        chain.fund(&b.pubkey(), 10 * SOL);
        let setup = SquadsSetup {
            members: [&a, &b]
                .map(|key| SquadsMember {
                    key: key.pubkey(),
                    permissions: PERMISSION_ALL,
                })
                .to_vec(),
            threshold: 2,
            time_lock: 0,
        };
        let squad = match program {
            SquadsProgram::V4 => {
                let create_key = KeypairSigner::new_random();
                let (squad, create) =
                    Squad::create_v4(&create_key.pubkey(), &a.pubkey(), &chain.treasury, &setup);
                chain.send(&[&a, &create_key], vec![create]).unwrap();
                squad
            }
            SquadsProgram::SmartAccount => {
                let config = chain.data(&program.program_config());
                let count = SquadsProgram::smart_account_count(&config).unwrap();
                let (squad, create) =
                    Squad::create_smart_account(count + 1, &a.pubkey(), &chain.treasury, &setup);
                chain.send(&[&a], vec![create]).unwrap();
                squad
            }
        };
        let vault = squad.vault(0);
        chain.fund(&vault, 3 * SOL);
        Self {
            chain,
            squad,
            vault,
            a,
            b,
            recipient: KeypairSigner::new_random().pubkey(),
        }
    }

    fn payment(&self, lamports: u64) -> Instruction {
        solana_system_interface::instruction::transfer(&self.vault, &self.recipient, lamports)
    }

    /// What the modal reads when it opens: the multisig, then its newest page.
    fn read(&self) -> (SquadState, Vec<ProposalEntry>) {
        let state = self
            .squad
            .parse_state(&self.chain.data(&self.squad.address))
            .unwrap();
        let indexes = page_indexes(state.transaction_index);
        let accounts: Vec<_> = page_accounts(&self.squad, &indexes)
            .iter()
            .map(|address| self.chain.account(address))
            .collect();
        let entries = read_page(&self.squad, &state, &indexes, &accounts);
        (state, entries)
    }

    /// Do what the modal offers `member` for transaction `index`.
    fn act(&mut self, member: &KeypairSigner, index: u64) -> ProposalAction {
        let (state, entries) = self.read();
        let entry = entries.iter().find(|entry| entry.index == index).unwrap();
        let action = available_action(&state, entry, &member.pubkey(), 0).unwrap();
        let instructions =
            action_instructions(&self.squad, entry, &member.pubkey(), action).unwrap();
        self.chain
            .send(&[member], instructions)
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
        action
    }
}

#[test]
fn another_member_finds_an_open_proposal_and_approves_and_executes_it() {
    for program in BOTH {
        let mut desk = Desk::new(program);
        let (a, b) = (desk.a.pubkey(), desk.b.pubkey());
        let payment = desk.payment(SOL);
        desk.chain
            .send(
                &[&desk.a],
                vec![
                    desk.squad
                        .create_transaction(1, 0, std::slice::from_ref(&payment), &a, &a)
                        .unwrap(),
                    desk.squad.create_proposal(1, &a, &a),
                    desk.squad.approve(1, &a),
                ],
            )
            .unwrap_or_else(|failure| panic!("{program:?}: {}", logs(&failure)));

        let (state, entries) = desk.read();
        assert_eq!((state.threshold, state.transaction_index), (2, 1));
        assert_eq!(entries.len(), 1, "{program:?}");
        let entry = &entries[0];
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Active, "{program:?}");
        assert_eq!(proposal.approved, vec![a], "{program:?}");
        // What is stored is what was proposed, read back from the account.
        let Stored::Vault(stored) = &entry.stored else {
            panic!("{program:?}: {:?}", entry.stored);
        };
        assert_eq!(stored.instructions, vec![payment], "{program:?}");
        assert_eq!((stored.vault, stored.creator), (desk.vault, a));

        // The proposer has voted and waits. The other member finishes it.
        assert_eq!(
            available_action(&state, entry, &a, 0),
            Err(NoAction::WaitingForOthers { missing: 1 })
        );
        assert_eq!(
            desk.act(&KeypairSigner::from_seed(&desk.b.seed_bytes()).unwrap(), 1),
            ProposalAction::ApproveAndExecute,
            "{program:?}"
        );
        assert_eq!(desk.chain.balance(&desk.recipient), SOL, "{program:?}");

        let (state, entries) = desk.read();
        let proposal = entries[0].proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Executed, "{program:?}");
        // The program keeps the list sorted by key.
        let mut both = vec![a, b];
        both.sort();
        assert_eq!(proposal.approved, both, "{program:?}");
        for member in [a, b] {
            assert_eq!(
                available_action(&state, &entries[0], &member, 0),
                Err(NoAction::Finished)
            );
        }
    }
}

#[test]
fn an_approval_that_is_not_the_last_leaves_the_proposal_open_for_the_next_member() {
    for program in BOTH {
        let mut desk = Desk::new(program);
        let a = desk.a.pubkey();
        // Two transactions, neither voted on yet.
        for index in 1..=2u64 {
            let payment = desk.payment(index * SOL / 4);
            desk.chain
                .send(
                    &[&desk.a],
                    vec![
                        desk.squad
                            .create_transaction(index, 0, &[payment], &a, &a)
                            .unwrap(),
                        desk.squad.create_proposal(index, &a, &a),
                    ],
                )
                .unwrap_or_else(|failure| panic!("{program:?}: {}", logs(&failure)));
        }
        // Newest first.
        let (_, entries) = desk.read();
        assert_eq!(
            entries.iter().map(|entry| entry.index).collect::<Vec<_>>(),
            vec![2, 1]
        );

        let (first, second) = (
            KeypairSigner::from_seed(&desk.b.seed_bytes()).unwrap(),
            KeypairSigner::from_seed(&desk.a.seed_bytes()).unwrap(),
        );
        assert_eq!(desk.act(&first, 2), ProposalAction::Approve, "{program:?}");
        assert_eq!(desk.chain.balance(&desk.recipient), 0, "{program:?}");
        assert_eq!(
            desk.act(&second, 2),
            ProposalAction::ApproveAndExecute,
            "{program:?}"
        );
        assert_eq!(desk.chain.balance(&desk.recipient), SOL / 2, "{program:?}");

        // Approve and execute as two transactions of the same member.
        assert_eq!(desk.act(&first, 1), ProposalAction::Approve, "{program:?}");
        desk.chain
            .send(&[&desk.a], vec![desk.squad.approve(1, &a)])
            .unwrap();
        let (state, entries) = desk.read();
        let older = entries.iter().find(|entry| entry.index == 1).unwrap();
        assert_eq!(
            older.proposal.as_ref().unwrap().status,
            ProposalStatus::Approved
        );
        assert_eq!(
            available_action(&state, older, &first.pubkey(), 0),
            Ok(ProposalAction::Execute)
        );
        assert_eq!(desk.act(&first, 1), ProposalAction::Execute, "{program:?}");
        assert_eq!(
            desk.chain.balance(&desk.recipient),
            SOL / 2 + SOL / 4,
            "{program:?}"
        );
    }
}
