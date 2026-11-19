//! How the real programs count rejections, against what the Proposals modal
//! shows.
//!
//! Three members may vote, a fourth may only propose and execute, and two
//! approvals are needed. The cutoff is then two: once two members reject,
//! two approvals can no longer be reached. Were the member without the vote
//! counted, it would be three.

use crate::support::{logs, Chain, SOL};
use solana_pubkey::Pubkey;
use wallet_core::proposals::{
    page_accounts, page_indexes, read_page, rejection_cutoff, standing, vote_of, ProposalEntry,
    Tally, Vote, Voter,
};
use wallet_core::squads::{
    ProposalStatus, Squad, SquadState, SquadsMember, SquadsProgram, SquadsSetup,
    PERMISSION_EXECUTE, PERMISSION_INITIATE, PERMISSION_VOTE,
};
use wallet_core::{KeypairSigner, WalletSigner};

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

struct Desk {
    chain: Chain,
    squad: Squad,
    /// May vote.
    voters: [KeypairSigner; 3],
    /// May propose and execute, not vote.
    proposer: KeypairSigner,
}

impl Desk {
    fn new(program: SquadsProgram) -> Self {
        let mut chain = Chain::new();
        let voters = [(); 3].map(|_| KeypairSigner::new_random());
        let proposer = KeypairSigner::new_random();
        for key in voters.iter().chain([&proposer]) {
            chain.fund(&key.pubkey(), 10 * SOL);
        }
        let mut members: Vec<SquadsMember> = voters
            .iter()
            .map(|key| SquadsMember {
                key: key.pubkey(),
                permissions: PERMISSION_INITIATE | PERMISSION_VOTE | PERMISSION_EXECUTE,
            })
            .collect();
        members.push(SquadsMember {
            key: proposer.pubkey(),
            permissions: PERMISSION_INITIATE | PERMISSION_EXECUTE,
        });
        let setup = SquadsSetup {
            members,
            threshold: 2,
            time_lock: 0,
            config_authority: None,
            rent_collector: None,
        };
        let creator = &voters[0];
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
                let (squad, create) = Squad::create_smart_account(
                    count + 1,
                    &creator.pubkey(),
                    &chain.treasury,
                    &setup,
                );
                chain.send(&[creator], vec![create]).unwrap();
                squad
            }
        };
        chain.fund(&squad.vault(0), SOL);
        Self {
            chain,
            squad,
            voters,
            proposer,
        }
    }

    /// The proposer stores a payment as transaction `index` and opens it.
    fn propose(&mut self, index: u64) {
        let proposer = self.proposer.pubkey();
        let payment = solana_system_interface::instruction::transfer(
            &self.squad.vault(0),
            &Pubkey::new_unique(),
            SOL / 10,
        );
        let instructions = vec![
            self.squad
                .create_transaction(index, 0, &[payment], &proposer, &proposer)
                .unwrap(),
            self.squad.create_proposal(index, &proposer, &proposer),
        ];
        self.chain
            .send(&[&self.proposer], instructions)
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    }

    fn reject(&mut self, voter: usize, index: u64) {
        let instruction = self.squad.reject(index, &self.voters[voter].pubkey());
        self.chain
            .send(&[&self.voters[voter]], vec![instruction])
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    }

    fn approve(&mut self, voter: usize, index: u64) {
        let instruction = self.squad.approve(index, &self.voters[voter].pubkey());
        self.chain
            .send(&[&self.voters[voter]], vec![instruction])
            .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    }

    /// What the modal reads: the multisig and transaction `index`.
    fn read(&self, index: u64) -> (SquadState, ProposalEntry) {
        let state = self
            .squad
            .parse_state(&self.chain.data(&self.squad.address))
            .unwrap();
        let indexes = page_indexes(state.transaction_index);
        let accounts: Vec<_> = page_accounts(&self.squad, &indexes)
            .iter()
            .map(|address| self.chain.account(address))
            .collect();
        let entry = read_page(
            &self.squad,
            &state,
            &indexes,
            &accounts,
            &Default::default(),
        )
        .into_iter()
        .find(|entry| entry.index == index)
        .unwrap();
        (state, entry)
    }

    fn key(&self, voter: usize) -> Pubkey {
        self.voters[voter].pubkey()
    }
}

fn sorted(mut keys: Vec<Pubkey>) -> Vec<Pubkey> {
    keys.sort();
    keys
}

#[test]
fn a_proposal_is_rejected_at_the_cutoff_of_the_members_that_may_vote() {
    for program in BOTH {
        let mut desk = Desk::new(program);
        desk.propose(1);
        let (state, entry) = desk.read(1);
        assert_eq!(rejection_cutoff(&state), 2, "{program:?}");
        let seen = standing(&state, &entry).unwrap();
        // The proposer may not vote, so it is not waited for.
        assert_eq!(
            sorted(seen.waiting),
            sorted((0..3).map(|at| desk.key(at)).collect()),
            "{program:?}"
        );

        desk.reject(0, 1);
        let (state, entry) = desk.read(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Active, "{program:?}");
        assert_eq!(vote_of(proposal, &desk.key(0)), Some(Vote::Rejected));
        let seen = standing(&state, &entry).unwrap();
        assert_eq!(
            seen.rejected,
            vec![Voter {
                key: desk.key(0),
                member: true
            }]
        );
        assert_eq!(
            seen.counts.rejected,
            Tally {
                count: 1,
                needed: 2
            },
            "{program:?}"
        );
        assert_eq!(sorted(seen.waiting), sorted(vec![desk.key(1), desk.key(2)]));

        // The second rejection is the last one, as the counts said.
        desk.reject(1, 1);
        let (state, entry) = desk.read(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.status, ProposalStatus::Rejected, "{program:?}");
        let seen = standing(&state, &entry).unwrap();
        assert!(seen.counts.rejected.reached(), "{program:?}");
        assert!(seen.waiting.is_empty(), "{program:?}");
    }
}

#[test]
fn approving_and_rejecting_take_back_each_other() {
    for program in BOTH {
        let mut desk = Desk::new(program);
        desk.propose(1);
        desk.approve(0, 1);
        desk.reject(0, 1);
        let (state, entry) = desk.read(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert!(proposal.approved.is_empty(), "{program:?}");
        assert_eq!(proposal.rejected, vec![desk.key(0)], "{program:?}");

        desk.approve(0, 1);
        let (_, entry) = desk.read(1);
        let proposal = entry.proposal.as_ref().unwrap();
        assert_eq!(proposal.approved, vec![desk.key(0)], "{program:?}");
        assert!(proposal.rejected.is_empty(), "{program:?}");
        assert_eq!(proposal.status, ProposalStatus::Active, "{program:?}");
        let seen = standing(&state, &entry).unwrap();
        assert_eq!(seen.counts.approved.missing(), 1, "{program:?}");
    }
}
