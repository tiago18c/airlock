//! When the accounts of a transaction can be closed: every status, stale or
//! not, vault transaction or change, with and without a v4 rent collector,
//! with and without a proposal account, on both programs.

use super::*;
use crate::wallets::squads::{CloseKind, SquadsProgram};

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];
const STATUSES: [ProposalStatus; 7] = [
    ProposalStatus::Draft,
    ProposalStatus::Active,
    ProposalStatus::Rejected,
    ProposalStatus::Approved,
    ProposalStatus::Executing,
    ProposalStatus::Executed,
    ProposalStatus::Cancelled,
];

fn squad_of(program: SquadsProgram) -> Squad {
    match program {
        SquadsProgram::V4 => Squad::v4(&pk(1)),
        SquadsProgram::SmartAccount => Squad::smart_account(7),
    }
}

/// The collector v4 multisigs set here.
fn collector() -> Pubkey {
    pk(40)
}

fn multisig(collecting: bool) -> SquadState {
    let mut state = state(1, 0, &[(pk(2), PERMISSION_ALL)]);
    state.rent_collector = collecting.then(collector);
    state
}

/// Transaction 5 with the proposal `status` (none for `None`), its accounts
/// as a page reads them: the proposal's rent paid by pk(41), the
/// transaction's by pk(42) on the smart account.
fn case(
    program: SquadsProgram,
    status: Option<ProposalStatus>,
    kind: CloseKind,
    stale: bool,
) -> ProposalEntry {
    let squad = squad_of(program);
    let mut entry = entry(&squad, status.unwrap_or(ProposalStatus::Active), &[]);
    if status.is_none() {
        entry.proposal = None;
    }
    if kind == CloseKind::Config {
        entry.stored = Stored::Other;
    }
    entry.stale = stale;
    let smart = program == SquadsProgram::SmartAccount;
    entry.accounts = EntryAccounts {
        proposal_lamports: if status.is_some() { 2_000 } else { 0 },
        transaction_lamports: 5_000,
        close_kind: Some(kind),
        proposal_rent_collector: (smart && status.is_some()).then(|| pk(41)),
        transaction_rent_collector: smart.then(|| pk(42)),
    };
    entry
}

/// What the programs' own tables say, for one case.
fn program_allows(status: Option<ProposalStatus>, kind: CloseKind, stale: bool) -> bool {
    match status {
        None => stale,
        Some(ProposalStatus::Draft | ProposalStatus::Active) => stale,
        Some(ProposalStatus::Approved) => kind == CloseKind::Config && stale,
        Some(ProposalStatus::Executing) => false,
        Some(ProposalStatus::Rejected | ProposalStatus::Executed | ProposalStatus::Cancelled) => {
            true
        }
    }
}

#[test]
fn every_status_stale_or_not_closes_as_the_programs_allow() {
    let statuses = STATUSES.map(Some).into_iter().chain([None]);
    for program in BOTH {
        for status in statuses.clone() {
            for kind in [CloseKind::Vault, CloseKind::Config] {
                for stale in [false, true] {
                    for collecting in [false, true] {
                        let entry = case(program, status, kind, stale);
                        let got = available_close(program, &multisig(collecting), &entry);
                        let what = format!(
                            "{program:?} {status:?} {kind:?} stale {stale} collecting {collecting}"
                        );
                        let allowed = program_allows(status, kind, stale);
                        // The smart account program needs no setting.
                        let needs_collector = program == SquadsProgram::V4 && !collecting;
                        match (allowed, needs_collector) {
                            (true, false) => {
                                let closing = got.unwrap_or_else(|why| panic!("{what}: {why}"));
                                assert_eq!(closing.index, 5);
                                assert_eq!(closing.kind, kind);
                                assert_eq!(
                                    closing.rent(),
                                    entry.accounts.proposal_lamports + 5_000,
                                    "{what}"
                                );
                            }
                            (true, true) => {
                                assert_eq!(got, Err(NoClose::NoRentCollector), "{what}")
                            }
                            (false, _) => {
                                let why = got.expect_err(&what);
                                let expected = match status {
                                    None => NoClose::NoProposal,
                                    Some(ProposalStatus::Draft) => NoClose::Draft,
                                    Some(ProposalStatus::Active) => NoClose::Open,
                                    Some(ProposalStatus::Approved) => NoClose::Approved,
                                    Some(ProposalStatus::Executing) => NoClose::Executing,
                                    _ => unreachable!("{what}"),
                                };
                                assert_eq!(why, expected, "{what}");
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn v4_returns_both_accounts_rent_to_the_multisigs_collector() {
    let entry = case(
        SquadsProgram::V4,
        Some(ProposalStatus::Executed),
        CloseKind::Vault,
        false,
    );
    let closing = available_close(SquadsProgram::V4, &multisig(true), &entry).unwrap();
    assert_eq!(closing.proposal_collector, collector());
    assert_eq!(closing.transaction_collector, collector());
    assert_eq!(closing.returns, vec![(collector(), 7_000)]);
    let squad = squad_of(SquadsProgram::V4);
    let instruction = closing.instruction(&squad);
    assert_eq!(instruction.program_id, squad.program.program_id());
    // No one signs a close: only the fee payer, outside it.
    assert!(instruction.accounts.iter().all(|meta| !meta.is_signer));
    assert_eq!(
        instruction
            .accounts
            .iter()
            .map(|meta| meta.pubkey)
            .collect::<Vec<_>>(),
        vec![
            squad.address,
            squad.proposal(5),
            squad.transaction(5),
            collector(),
            crate::ids::SYSTEM_PROGRAM_ID
        ]
    );
}

#[test]
fn the_smart_account_returns_each_accounts_rent_to_whoever_paid_it() {
    let program = SquadsProgram::SmartAccount;
    let entry = case(
        program,
        Some(ProposalStatus::Rejected),
        CloseKind::Config,
        false,
    );
    // The multisig's collector is no matter here.
    let closing = available_close(program, &multisig(false), &entry).unwrap();
    assert_eq!(closing.proposal_collector, pk(41));
    assert_eq!(closing.transaction_collector, pk(42));
    assert_eq!(closing.returns, vec![(pk(41), 2_000), (pk(42), 5_000)]);
    let squad = squad_of(program);
    let instruction = closing.instruction(&squad);
    assert!(instruction.accounts.iter().all(|meta| !meta.is_signer));
    assert_eq!(instruction.accounts[3].pubkey, pk(41));
    assert_eq!(instruction.accounts[4].pubkey, pk(42));

    // One payer for both: one line of rent.
    let mut same = entry.clone();
    same.accounts.proposal_rent_collector = Some(pk(42));
    let closing = available_close(program, &multisig(false), &same).unwrap();
    assert_eq!(closing.returns, vec![(pk(42), 7_000)]);

    // No proposal: the transaction's payer stands in for the proposal's,
    // which the program does not check then.
    let alone = case(program, None, CloseKind::Vault, true);
    let closing = available_close(program, &multisig(false), &alone).unwrap();
    assert_eq!(closing.proposal_collector, pk(42));
    assert_eq!(closing.returns, vec![(pk(42), 5_000)]);
}

#[test]
fn what_is_gone_or_not_closed_here_is_said() {
    for program in BOTH {
        let state = multisig(true);
        // Closed: both accounts gone.
        let mut closed = case(program, None, CloseKind::Vault, false);
        closed.stored = Stored::Missing;
        closed.accounts = EntryAccounts::default();
        assert_eq!(
            available_close(program, &state, &closed),
            Err(NoClose::Closed)
        );
        assert!(!NoClose::Closed.worth_saying(&closed));
        // A proposal whose stored transaction is gone stays.
        let mut gone = case(
            program,
            Some(ProposalStatus::Executed),
            CloseKind::Vault,
            false,
        );
        gone.stored = Stored::Missing;
        gone.accounts.transaction_lamports = 0;
        gone.accounts.close_kind = None;
        assert_eq!(available_close(program, &state, &gone), Err(NoClose::Gone));
        // A batch.
        let mut batch = case(
            program,
            Some(ProposalStatus::Executed),
            CloseKind::Vault,
            false,
        );
        batch.stored = Stored::Other;
        batch.accounts.close_kind = None;
        assert_eq!(
            available_close(program, &state, &batch),
            Err(NoClose::NotClosedHere)
        );
    }
}

/// Only a finished proposal of a v4 multisig without a rent collector says
/// why it cannot be closed: nothing else does.
#[test]
fn only_a_missing_rent_collector_on_a_finished_proposal_is_said() {
    let program = SquadsProgram::V4;
    for status in STATUSES {
        for stale in [false, true] {
            let entry = case(program, Some(status), CloseKind::Vault, stale);
            let finished = matches!(
                status,
                ProposalStatus::Executed | ProposalStatus::Rejected | ProposalStatus::Cancelled
            );
            assert_eq!(
                NoClose::NoRentCollector.worth_saying(&entry),
                finished,
                "{status:?} stale {stale}"
            );
            for other in [
                NoClose::Approved,
                NoClose::Open,
                NoClose::Draft,
                NoClose::NoProposal,
                NoClose::Executing,
                NoClose::Gone,
                NoClose::NotClosedHere,
                NoClose::Closed,
            ] {
                assert!(!other.worth_saying(&entry), "{other:?} {status:?}");
            }
        }
    }
}

#[test]
fn closing_many_lists_each_close_and_sums_the_rent_by_collector() {
    let program = SquadsProgram::V4;
    let state = multisig(true);
    let entries: Vec<ProposalEntry> = [
        Some(ProposalStatus::Executed),
        Some(ProposalStatus::Active),
        Some(ProposalStatus::Rejected),
        Some(ProposalStatus::Approved),
        Some(ProposalStatus::Cancelled),
    ]
    .into_iter()
    .enumerate()
    .map(|(at, status)| ProposalEntry {
        index: 9 - at as u64,
        ..case(program, status, CloseKind::Vault, false)
    })
    .collect();
    let closings = closable(program, &state, &entries);
    assert_eq!(
        closings
            .iter()
            .map(|closing| closing.index)
            .collect::<Vec<_>>(),
        vec![9, 7, 5]
    );
    assert_eq!(rent_returned(&closings), vec![(collector(), 21_000)]);
    let request = ProposalRequest::Close(closings);
    assert!(!request.executes());
    let squad = squad_of(program);
    let instructions = request.instructions(&squad, &entries[0]).unwrap();
    assert_eq!(instructions.len(), 3);
    assert!(instructions
        .iter()
        .flat_map(|instruction| &instruction.accounts)
        .all(|meta| !meta.is_signer));
    assert_eq!(
        ProposalRequest::Close(Vec::new()).instructions(&squad, &entries[0]),
        None
    );
}
