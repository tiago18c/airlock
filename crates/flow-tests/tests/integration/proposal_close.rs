//! Closing a proposal's accounts to return their rent, on the path a person
//! takes, against the real programs: the Proposals modal reads the page and
//! offers the close, it is compiled as the modal does with the fee payer
//! alone signing, Review and Sign simulates and fits it, it is signed and
//! submitted, the rent reaches the collector, and the modal, opened again,
//! reads the number as closed.
//!
//! What the programs allow, proven here on both:
//!
//! - No member signs a close: a wallet that is no member pays the fee and
//!   sends it, and the rent goes to the collector, not to it.
//! - Executed, rejected and cancelled proposals close; an approved vault
//!   transaction is refused (`InvalidProposalStatus`), also once stale; an
//!   open one is refused until a change to the multisig makes it stale, and
//!   so is a stored transaction with no proposal.
//! - A change to the multisig closes through the sibling instruction, and
//!   an approved change closes once stale.
//! - v4 sends the rent of both accounts to the multisig's rent collector,
//!   and refuses a close on a multisig without one
//!   (`RentReclamationDisabled`) until a change to the multisig sets one.
//!   The smart account program returns each account's rent to whoever paid
//!   it, and refuses another collector (`InvalidRentCollector`).
//! - Several closes go in one transaction.

use crate::proposal_execute::{funded, press_sign, run, Team};
use crate::support::{logs, Chain, SOL};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use wallet_core::adapt::plan_adaptation;
use wallet_core::proposals::{
    available_close, closable, execute_instruction, EntryAccounts, NoClose, ProposalEntry,
    ProposalRequest, Stored, CLOSES_PER_TRANSACTION,
};
use wallet_core::squads::{
    CloseKind, ConfigAction, ProposalStatus, SquadState, SquadsMember, SquadsProgram,
    PERMISSION_ALL, PERMISSION_VOTE,
};
use wallet_core::{
    decode_v1_transaction, KeypairSigner, WalletRecord, WalletSigner, MAX_TRANSACTION_SIZE,
};

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

/// Three members that may do everything, 2 of 3, and the proposer. On v4
/// `collecting` gives the multisig a rent collector of its own, which is
/// returned; on the smart account the rent goes back to the proposer, who
/// pays for every account here.
fn three(program: SquadsProgram, collecting: bool) -> (Team, [KeypairSigner; 3], Pubkey) {
    let mut chain = Chain::new();
    let keys = [(); 3].map(|_| funded(&mut chain));
    let members =
        [keys[0].pubkey(), keys[1].pubkey(), keys[2].pubkey()].map(|key| (key, PERMISSION_ALL));
    let collector = KeypairSigner::new_random().pubkey();
    let team = Team::with(chain, program, &members, 2, |setup| {
        if program == SquadsProgram::V4 && collecting {
            setup.rent_collector = Some(collector);
        }
    });
    let collector = match program {
        SquadsProgram::V4 => collector,
        SquadsProgram::SmartAccount => team.proposer.pubkey(),
    };
    (team, keys, collector)
}

fn send(team: &Team, keys: &[&KeypairSigner], instructions: Vec<Instruction>) {
    let sent = team.node.chain().send(keys, instructions);
    sent.unwrap_or_else(|failure| panic!("{:?}: {}", team.squad.program, logs(&failure)));
}

/// The program refused `instructions`, signed by `keys`, with `error`.
fn refused(team: &Team, keys: &[&KeypairSigner], instructions: Vec<Instruction>, error: &str) {
    let failure = team.node.chain().send(keys, instructions).unwrap_err();
    let logs = logs(&failure);
    assert!(logs.contains(error), "{:?}: {logs}", team.squad.program);
}

/// Proposal `index`, a payment, approved by the first two members.
fn approved(team: &Team, index: u64, keys: &[KeypairSigner; 3]) {
    team.propose(index, &[team.payment(SOL / 100)]);
    team.approve(index, &keys[0]);
    team.approve(index, &keys[1]);
}

fn executed(team: &Team, index: u64, keys: &[KeypairSigner; 3]) {
    approved(team, index, keys);
    team.execute_as(index, &keys[0].pubkey(), &[&team.fees, &keys[0]]);
}

fn rejected(team: &Team, index: u64, keys: &[KeypairSigner; 3]) {
    team.propose(index, &[team.payment(SOL / 100)]);
    for key in &keys[..2] {
        send(team, &[key], vec![team.squad.reject(index, &key.pubkey())]);
    }
}

fn cancelled(team: &Team, index: u64, keys: &[KeypairSigner; 3]) {
    approved(team, index, keys);
    for key in &keys[..2] {
        send(team, &[key], vec![team.squad.cancel(index, &key.pubkey())]);
    }
}

/// Change `index` to the multisig, `actions`, proposed, approved by the
/// first two members and executed by the first, the fee payer paying.
fn changed(team: &Team, index: u64, keys: &[KeypairSigner; 3], actions: &[ConfigAction]) {
    let by = team.proposer.pubkey();
    let store = team.squad.create_change(index, actions, &by, &by).unwrap();
    team.propose_with(index, store);
    team.approve(index, &keys[0]);
    team.approve(index, &keys[1]);
    let (_, entry) = team.modal(index);
    let execute =
        execute_instruction(&team.squad, &entry, &keys[0].pubkey(), &team.fees.pubkey()).unwrap();
    send(team, &[&team.fees, &keys[0]], vec![execute]);
}

/// A change that makes every proposal made so far stale: a member added.
fn stale_all(team: &Team, index: u64, keys: &[KeypairSigner; 3]) {
    let newcomer = KeypairSigner::new_random().pubkey();
    changed(
        team,
        index,
        keys,
        &[ConfigAction::AddMember(SquadsMember {
            key: newcomer,
            permissions: PERMISSION_VOTE,
        })],
    );
}

/// The multisig and every transaction from the newest down, as the modal
/// reads them with Older pressed until there is nothing older: a page that
/// holds nothing but closed numbers still leads further back.
fn page(team: &Team) -> (SquadState, Vec<ProposalEntry>) {
    let read = |from| {
        run(wallet_flows::squads::load_page(
            &team.node,
            &team.squad,
            from,
        ))
        .unwrap_or_else(|err| panic!("{err}"))
    };
    let (state, mut entries) = read(None);
    while let Some(last) = entries
        .last()
        .map(|entry| entry.index)
        .filter(|last| *last > 1)
    {
        let (_, older) = read(Some(last - 1));
        assert!(!older.is_empty());
        entries.extend(older);
    }
    (state, entries)
}

fn rent_of(entry: &ProposalEntry) -> u64 {
    let EntryAccounts {
        proposal_lamports,
        transaction_lamports,
        ..
    } = entry.accounts.clone();
    proposal_lamports + transaction_lamports
}

/// Close `indexes` as the modal does, with `payer`, which is no member,
/// alone signing, all the way to the chain. Answers the signed wire.
fn close(team: &Team, indexes: &[u64], payer: &KeypairSigner) -> Vec<u8> {
    let (state, entries) = page(team);
    let closings: Vec<_> = closable(team.squad.program, &state, &entries)
        .into_iter()
        .filter(|closing| indexes.contains(&closing.index))
        .collect();
    assert_eq!(closings.len(), indexes.len(), "{:?}", team.squad.program);
    let entry = entries.first().unwrap();
    let request = ProposalRequest::Close(closings);
    let (wire, _) = run(wallet_flows::squads::compile_request(
        &team.node,
        &team.squad,
        entry,
        &request,
        payer.pubkey(),
        |_| None,
        None,
    ))
    .unwrap_or_else(|err| panic!("{err}"));
    let tx = decode_v1_transaction(&wire).unwrap();
    assert_eq!(
        tx.signers
            .iter()
            .map(|signer| signer.pubkey)
            .collect::<Vec<_>>(),
        vec![payer.pubkey()],
        "{:?}: the fee payer alone signs a close",
        team.squad.program
    );
    // Nothing to rebuild for a member without a key: no member is in it.
    let wallets = vec![WalletRecord::from_keypair_seed("Fees", payer.seed_bytes())];
    assert!(plan_adaptation(&tx, &wallets, None).unwrap().is_none());
    let wire = team.review(&wire);
    let used = team
        .node
        .chain()
        .simulate_wire(&wire)
        .map(|meta| meta.compute_units_consumed)
        .unwrap_or_else(|failure| panic!("{}", logs(&failure)));
    println!(
        "{:?}: {} close(s) use {used} compute units",
        team.squad.program,
        indexes.len()
    );
    // Well within the 1.4 million a transaction may use.
    assert!(used < 600_000, "{:?}: {used}", team.squad.program);
    let signed = press_sign(Some(&team.node), &wire, &[payer]);
    team.submit(&signed.wire);
    signed.wire
}

/// The modal, opened again, reads `index` as closed: neither account is
/// left, and nothing more can be closed or done.
fn assert_closed(team: &Team, index: u64) {
    let (state, entries) = page(team);
    let entry = entries.iter().find(|entry| entry.index == index).unwrap();
    assert_eq!(entry.stored, Stored::Missing, "{:?}", team.squad.program);
    assert_eq!(entry.proposal, None);
    assert_eq!(rent_of(entry), 0);
    assert_eq!(
        available_close(team.squad.program, &state, entry),
        Err(NoClose::Closed)
    );
    assert!(wallet_core::proposals::standing(&state, entry).is_none());
}

/// Executed, rejected and cancelled proposals are closed by a wallet that
/// is no member, the rent of both accounts reaching the collector and none
/// of it the sender. The modal then reads them as closed, and a page of
/// nothing but closed numbers still reads.
#[test]
fn finished_proposals_are_closed_by_anyone_and_the_rent_reaches_the_collector() {
    for program in BOTH {
        let (team, keys, collector) = three(program, true);
        executed(&team, 1, &keys);
        rejected(&team, 2, &keys);
        cancelled(&team, 3, &keys);
        let outsider = funded(&mut team.node.chain());
        for index in [1, 2, 3] {
            let (state, entries) = page(&team);
            let entry = entries.iter().find(|entry| entry.index == index).unwrap();
            let closing = available_close(program, &state, entry)
                .unwrap_or_else(|why| panic!("{program:?} #{index}: {why}"));
            assert_eq!(closing.kind, CloseKind::Vault);
            assert_eq!(closing.returns, vec![(collector, rent_of(entry))]);
            let before = team.node.chain().balance(&collector);
            let paid = team.node.chain().balance(&outsider.pubkey());
            close(&team, &[index], &outsider);
            assert_eq!(
                team.node.chain().balance(&collector),
                before + rent_of(entry),
                "{program:?} #{index}: the rent of both accounts"
            );
            assert!(
                team.node.chain().balance(&outsider.pubkey()) < paid,
                "{program:?}: the sender only pays the fee"
            );
            assert_closed(&team, index);
        }
        let (_, entries) = page(&team);
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|entry| entry.stored == Stored::Missing));
    }
}

/// An approved vault transaction is never closed, stale or not: it can
/// still be executed. Once executed, it closes.
#[test]
fn an_approved_vault_transaction_is_refused_also_when_stale() {
    for program in BOTH {
        let (team, keys, collector) = three(program, true);
        approved(&team, 1, &keys);
        for stale in [false, true] {
            if stale {
                stale_all(&team, 2, &keys);
            }
            let (state, entries) = page(&team);
            let entry = entries.iter().find(|entry| entry.index == 1).unwrap();
            assert_eq!(entry.stale, stale);
            assert_eq!(
                available_close(program, &state, entry),
                Err(NoClose::Approved),
                "{program:?} stale {stale}"
            );
            assert!(!NoClose::Approved.worth_saying(entry));
            let close = team
                .squad
                .close(1, CloseKind::Vault, &collector, &collector);
            refused(&team, &[&team.fees], vec![close], "InvalidProposalStatus");
        }
        team.execute_as(1, &keys[0].pubkey(), &[&team.fees, &keys[0]]);
        close(&team, &[1], &team.fees);
        assert_closed(&team, 1);
    }
}

/// An open proposal and a stored transaction with no proposal are refused
/// until a change to the multisig makes them stale; then they close, and so
/// does the change, through the sibling instruction.
#[test]
fn an_open_proposal_or_one_never_opened_closes_once_stale_and_so_does_the_change() {
    for program in BOTH {
        let (team, keys, collector) = three(program, true);
        team.propose(1, &[team.payment(SOL / 100)]);
        team.approve(1, &keys[0]);
        // Stored, never opened.
        let by = team.proposer.pubkey();
        let store = team
            .squad
            .create_transaction(2, 0, &[team.payment(SOL / 100)], &by, &by)
            .unwrap();
        send(&team, &[&team.proposer], vec![store]);

        let (state, entries) = page(&team);
        for (index, why) in [(1, NoClose::Open), (2, NoClose::NoProposal)] {
            let entry = entries.iter().find(|entry| entry.index == index).unwrap();
            assert_eq!(
                available_close(program, &state, entry),
                Err(why),
                "{program:?} #{index}"
            );
            let close = team
                .squad
                .close(index, CloseKind::Vault, &collector, &collector);
            refused(&team, &[&team.fees], vec![close], "InvalidProposalStatus");
        }

        stale_all(&team, 3, &keys);
        let (state, entries) = page(&team);
        let alone = entries.iter().find(|entry| entry.index == 2).unwrap();
        assert_eq!(alone.accounts.proposal_lamports, 0);
        let closing = available_close(program, &state, alone).unwrap();
        assert_eq!(closing.returns, vec![(collector, rent_of(alone))]);
        let change = entries.iter().find(|entry| entry.index == 3).unwrap();
        assert_eq!(
            change.proposal.as_ref().unwrap().status,
            ProposalStatus::Executed
        );
        assert_eq!(
            available_close(program, &state, change).unwrap().kind,
            CloseKind::Config
        );
        let before = team.node.chain().balance(&collector);
        let rent: u64 = [1, 2, 3]
            .iter()
            .map(|index| rent_of(entries.iter().find(|entry| entry.index == *index).unwrap()))
            .sum();
        // The open one, the one never opened and the change, in one go.
        close(&team, &[3, 2, 1], &team.fees);
        assert_eq!(team.node.chain().balance(&collector), before + rent);
        for index in [1, 2, 3] {
            assert_closed(&team, index);
        }
    }
}

/// An approved change to the multisig closes once a later change made it
/// stale: it can then never be executed.
#[test]
fn an_approved_change_closes_once_stale() {
    for program in BOTH {
        let (team, keys, collector) = three(program, true);
        let by = team.proposer.pubkey();
        let store = team
            .squad
            .create_change(1, &[ConfigAction::ChangeThreshold(3)], &by, &by)
            .unwrap();
        team.propose_with(1, store);
        team.approve(1, &keys[0]);
        team.approve(1, &keys[1]);
        let (state, entries) = page(&team);
        assert_eq!(
            available_close(program, &state, &entries[0]),
            Err(NoClose::Approved)
        );
        let close = team
            .squad
            .close(1, CloseKind::Config, &collector, &collector);
        refused(&team, &[&team.fees], vec![close], "InvalidProposalStatus");
        stale_all(&team, 2, &keys);
        close_and_check(&team, 1, collector);
    }
}

fn close_and_check(team: &Team, index: u64, collector: Pubkey) {
    let (_, entries) = page(team);
    let entry = entries.iter().find(|entry| entry.index == index).unwrap();
    let before = team.node.chain().balance(&collector);
    close(team, &[index], &team.fees);
    assert_eq!(
        team.node.chain().balance(&collector),
        before + rent_of(entry)
    );
    assert_closed(team, index);
}

/// A v4 multisig without a rent collector cannot have its accounts closed:
/// the program refuses whatever collector is named. A change to the
/// multisig that sets one, which does not make anything stale, opens them
/// up, the change included.
#[test]
fn a_v4_multisig_without_a_rent_collector_closes_nothing_until_a_change_sets_one() {
    let program = SquadsProgram::V4;
    let (team, keys, _) = three(program, false);
    executed(&team, 1, &keys);
    let (state, entries) = page(&team);
    assert_eq!(state.rent_collector, None);
    assert_eq!(
        available_close(program, &state, &entries[0]),
        Err(NoClose::NoRentCollector)
    );
    let anyone = KeypairSigner::new_random().pubkey();
    let close = team.squad.close(1, CloseKind::Vault, &anyone, &anyone);
    refused(&team, &[&team.fees], vec![close], "RentReclamationDisabled");

    let collector = KeypairSigner::new_random().pubkey();
    changed(
        &team,
        2,
        &keys,
        &[ConfigAction::SetRentCollector(Some(collector))],
    );
    let (state, entries) = page(&team);
    assert_eq!(state.rent_collector, Some(collector));
    assert!(entries.iter().all(|entry| !entry.stale));
    assert_eq!(closable(program, &state, &entries).len(), 2);
    // Another address than the collector is refused.
    let wrong = team.squad.close(1, CloseKind::Vault, &anyone, &anyone);
    refused(&team, &[&team.fees], vec![wrong], "InvalidRentCollector");
    close_and_check(&team, 1, collector);
    close_and_check(&team, 2, collector);
}

/// The smart account program returns each account's rent to whoever paid
/// it, and refuses any other address: the app names the ones each account
/// recorded.
#[test]
fn the_smart_account_returns_rent_to_whoever_paid_it() {
    let program = SquadsProgram::SmartAccount;
    let (team, keys, _) = three(program, false);
    // The proposer stores it, a member pays for the proposal.
    let by = team.proposer.pubkey();
    let store = team
        .squad
        .create_transaction(1, 0, &[team.payment(SOL / 100)], &by, &by)
        .unwrap();
    let open = team.squad.create_proposal(1, &by, &keys[2].pubkey());
    send(&team, &[&team.proposer, &keys[2]], vec![store, open]);
    for key in &keys[..2] {
        send(&team, &[key], vec![team.squad.reject(1, &key.pubkey())]);
    }
    let (state, entries) = page(&team);
    let entry = &entries[0];
    let closing = available_close(program, &state, entry).unwrap();
    assert_eq!(closing.proposal_collector, keys[2].pubkey());
    assert_eq!(closing.transaction_collector, by);
    assert_eq!(
        closing.returns,
        vec![
            (keys[2].pubkey(), entry.accounts.proposal_lamports),
            (by, entry.accounts.transaction_lamports)
        ]
    );
    let anyone = KeypairSigner::new_random().pubkey();
    for (proposal, transaction) in [(anyone, by), (keys[2].pubkey(), anyone)] {
        let wrong = team
            .squad
            .close(1, CloseKind::Vault, &proposal, &transaction);
        refused(&team, &[&team.fees], vec![wrong], "InvalidRentCollector");
    }
    let before = [keys[2].pubkey(), by].map(|key| team.node.chain().balance(&key));
    close(&team, &[1], &team.fees);
    assert_eq!(
        team.node.chain().balance(&keys[2].pubkey()),
        before[0] + entry.accounts.proposal_lamports
    );
    assert_eq!(
        team.node.chain().balance(&by),
        before[1] + entry.accounts.transaction_lamports
    );
    assert_closed(&team, 1);
}

/// As many closes as one transaction takes go in one, and fit it with room
/// to spare; the rest are left for the next.
#[test]
fn several_closes_go_in_one_transaction() {
    for program in BOTH {
        let (team, keys, collector) = three(program, true);
        let count = CLOSES_PER_TRANSACTION as u64 + 1;
        for index in 1..=count {
            rejected(&team, index, &keys);
        }
        let (state, entries) = page(&team);
        let all = closable(program, &state, &entries);
        assert_eq!(all.len() as u64, count);
        let first: Vec<u64> = all
            .iter()
            .take(CLOSES_PER_TRANSACTION)
            .map(|closing| closing.index)
            .collect();
        let rent: u64 = entries
            .iter()
            .filter(|entry| first.contains(&entry.index))
            .map(rent_of)
            .sum();
        let before = team.node.chain().balance(&collector);
        let wire = close(&team, &first, &team.fees);
        println!(
            "{program:?}: {} closes in {} of {MAX_TRANSACTION_SIZE} bytes",
            first.len(),
            wire.len()
        );
        assert!(wire.len() * 2 < MAX_TRANSACTION_SIZE, "{program:?}");
        assert_eq!(team.node.chain().balance(&collector), before + rent);
        let (state, entries) = page(&team);
        let left = closable(program, &state, &entries);
        assert_eq!(
            left.iter().map(|closing| closing.index).collect::<Vec<_>>(),
            vec![1]
        );
        for index in first {
            assert_closed(&team, index);
        }
    }
}
