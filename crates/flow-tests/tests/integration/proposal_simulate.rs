//! Simulating what a proposal would run, against the real programs: the
//! stored instructions put in a transaction of their own, the vault marked
//! as signing, another wallet paying the fee, simulated with signature
//! checks off (`wallet_core::squads::VaultRun`). What the simulation says
//! changes must be what the execution then changes, on both programs.
//!
//! The local chain simulates as a node asked with `sigVerify: false` and
//! `replaceRecentBlockhash: true` does: no signature is checked, the
//! blockhash is not, and the balances after are answered. That a real node
//! takes a program-derived address in the signer section as having signed
//! was checked by hand on devnet; these tests prove the transaction built and
//! what is read from the answer, not the RPC call itself.

use crate::proposal_execute::{funded, run, Team};
use crate::support::vector::Desk;
use crate::support::{logs, SOL};
use solana_pubkey::Pubkey;
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::proposals::{ProposalEntry, Stored};
use wallet_core::simulate::{BalanceChange, Simulation};
use wallet_core::squads::{
    Squad, SquadsMember, SquadsProgram, SquadsSetup, StoredTransaction, PERMISSION_ALL,
};
use wallet_core::{
    decode_v1_transaction, stored_for_later, KeypairSigner, TxFormat, WalletId, WalletRecord,
    WalletSigner, SYSTEM_PROGRAM_ID,
};
use wallet_flows::simulate::simulate_vault_run;

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

/// What the row's Simulate shows for `entry`, with `fee_payer` paying.
fn simulate_entry(team: &Team, entry: &ProposalEntry, fee_payer: Pubkey) -> Simulation {
    let Stored::Vault(stored) = &entry.stored else {
        panic!("not a vault transaction: {:?}", entry.stored);
    };
    let run_now = stored.run_now(fee_payer).unwrap();
    run(simulate_vault_run(&team.node, &run_now)).unwrap_or_else(|err| panic!("{err}"))
}

fn change(pubkey: Pubkey, before: u64, after: u64) -> BalanceChange {
    BalanceChange {
        pubkey,
        before,
        after,
    }
}

/// A stored payment, open for votes and then approved: the simulation
/// changes the vault and the recipient, and nothing of the wallet paying its
/// fee; the execution then changes exactly that.
#[test]
fn a_stored_payment_simulates_as_its_execution_then_runs() {
    for program in BOTH {
        let mut chain = crate::support::Chain::new();
        let [a, b] = [(); 2].map(|_| funded(&mut chain));
        let members = [a.pubkey(), b.pubkey()].map(|key| (key, PERMISSION_ALL));
        let team = Team::on(chain, program, &members, 2, 0, None);
        team.propose(1, &[team.payment(SOL / 4)]);
        let vault_before = team.node.chain().balance(&team.vault);
        let fees_before = team.node.chain().balance(&team.fees.pubkey());

        let (_, open) = team.modal(1);
        let simulated = simulate_entry(&team, &open, team.fees.pubkey());
        assert_eq!(simulated.error, None, "{program:?}: {:?}", simulated.logs);
        let expected = vec![
            change(team.vault, vault_before, vault_before - SOL / 4),
            change(team.recipient, 0, SOL / 4),
        ];
        assert_eq!(simulated.balance_changes, expected, "{program:?}");
        // Nothing was sent.
        assert_eq!(team.node.chain().balance(&team.fees.pubkey()), fees_before);
        assert_eq!(team.node.chain().balance(&team.recipient), 0);

        // Approved, it simulates the same, and the execution does just that.
        team.approve(1, &a);
        team.approve(1, &b);
        let (_, approved) = team.modal(1);
        let again = simulate_entry(&team, &approved, team.fees.pubkey());
        assert_eq!(again.balance_changes, expected, "{program:?}");
        team.execute_as(1, &a.pubkey(), &[&team.fees, &a]);
        team.assert_executed(1, &a.pubkey());
        let chain = team.node.chain();
        assert_eq!(chain.balance(&team.vault), vault_before - SOL / 4);
        assert_eq!(chain.balance(&team.recipient), SOL / 4);
    }
}

/// A draft and an open proposal storing the same payment simulate alike.
#[test]
fn a_draft_and_an_open_proposal_simulate_alike() {
    for program in BOTH {
        let mut chain = crate::support::Chain::new();
        let a = funded(&mut chain);
        let team = Team::on(chain, program, &[(a.pubkey(), PERMISSION_ALL)], 1, 0, None);
        let by = team.proposer.pubkey();
        let payment = team.payment(SOL / 10);
        let store = team
            .squad
            .create_transaction(1, 0, std::slice::from_ref(&payment), &by, &by)
            .unwrap();
        let draft = team.squad.create_draft(1, &by, &by);
        let sent = team
            .node
            .chain()
            .send(&[&team.proposer], vec![store, draft]);
        sent.unwrap_or_else(|failure| panic!("{}", logs(&failure)));
        team.propose(2, &[payment]);

        let (_, drafted) = team.modal(1);
        let (_, open) = team.modal(2);
        let draft_says = simulate_entry(&team, &drafted, team.fees.pubkey());
        let open_says = simulate_entry(&team, &open, team.fees.pubkey());
        assert_eq!(draft_says.error, None, "{program:?}");
        assert_eq!(draft_says.balance_changes, open_says.balance_changes);
        assert_eq!(draft_says.balance_changes.len(), 2, "{program:?}");
    }
}

/// A payment larger than the vault holds fails in simulation with the
/// System program's own error, on the stored instruction that would fail,
/// and no balance is said to change.
#[test]
fn a_stored_payment_the_vault_cannot_afford_fails_with_the_program_s_error() {
    for program in BOTH {
        let mut chain = crate::support::Chain::new();
        let a = funded(&mut chain);
        let team = Team::on(chain, program, &[(a.pubkey(), PERMISSION_ALL)], 1, 0, None);
        // A payment it can afford first, so the failing one is the second.
        team.propose(1, &[team.payment(SOL), team.payment(10 * SOL)]);
        let (_, open) = team.modal(1);
        let simulated = simulate_entry(&team, &open, team.fees.pubkey());
        let error = simulated.error.expect("it fails");
        assert_eq!(error.instruction, Some(1), "{program:?}");
        let custom = error.custom.as_deref().expect("a program's error");
        assert_eq!((custom.code, custom.program), (1, Some(SYSTEM_PROGRAM_ID)));
        assert!(simulated.balance_changes.is_empty());
        assert!(
            simulated
                .logs
                .iter()
                .any(|line| line.contains("insufficient lamports")),
            "{program:?}: {:?}",
            simulated.logs
        );
    }
}

/// A stored transaction that reads its accounts from lookup tables: one
/// small enough is simulated as a v1 transaction listing them, and the
/// execution then does what it said; one naming more accounts than a v1
/// transaction may list is simulated as a v0 transaction reading the same
/// table.
#[test]
fn a_stored_transaction_that_reads_lookup_tables_simulates() {
    for program in BOTH {
        let mut chain = crate::support::Chain::new();
        let a = funded(&mut chain);
        let (first, second) = (
            KeypairSigner::new_random().pubkey(),
            KeypairSigner::new_random().pubkey(),
        );
        let filler = KeypairSigner::new_random().pubkey();
        let one = chain.plant_lookup_table(&[filler, first]);
        let other = chain.plant_lookup_table(&[second]);
        let many: Vec<Pubkey> = (0..70)
            .map(|_| KeypairSigner::new_random().pubkey())
            .collect();
        let wide = chain.plant_lookup_table(&many);
        let team = Team::on(chain, program, &[(a.pubkey(), PERMISSION_ALL)], 1, 0, None);
        let by = team.proposer.pubkey();

        let payments = vec![
            solana_system_interface::instruction::transfer(&team.vault, &first, SOL / 4),
            solana_system_interface::instruction::transfer(&team.vault, &second, SOL / 2),
        ];
        let tables = [(one, vec![filler, first]), (other, vec![second])];
        let store = team
            .squad
            .create_transaction_with_tables(1, 0, &payments, &tables, 0, &by, &by)
            .unwrap();
        team.propose_with(1, store);
        let (_, entry) = team.modal(1);
        let Stored::Vault(stored) = &entry.stored else {
            panic!("{program:?}: {:?}", entry.stored);
        };
        assert_eq!(stored.lookup_tables, vec![one, other]);
        let simulated = simulate_entry(&team, &entry, team.fees.pubkey());
        assert_eq!(simulated.error, None, "{program:?}: {:?}", simulated.logs);
        let mut said: Vec<(Pubkey, u64)> = simulated
            .balance_changes
            .iter()
            .filter(|change| change.pubkey != team.vault)
            .map(|change| (change.pubkey, change.after - change.before))
            .collect();
        said.sort();
        team.approve(1, &a);
        team.execute_as(1, &a.pubkey(), &[&team.fees, &a]);
        let chain = team.node.chain();
        let mut done = vec![
            (first, chain.balance(&first)),
            (second, chain.balance(&second)),
        ];
        drop(chain);
        done.sort();
        assert_eq!(said, done, "{program:?}");

        // A transfer to the first of 70 addresses from one table, naming
        // the other 69 as accounts the System program does not read.
        let mut transfer =
            solana_system_interface::instruction::transfer(&team.vault, &many[0], SOL / 8);
        transfer.accounts.extend(
            many[1..]
                .iter()
                .map(|key| solana_instruction::AccountMeta::new_readonly(*key, false)),
        );
        let store = team
            .squad
            .create_transaction_with_tables(
                2,
                0,
                std::slice::from_ref(&transfer),
                &[(wide, many.clone())],
                0,
                &by,
                &by,
            )
            .unwrap();
        team.propose_with(2, store);
        let (_, entry) = team.modal(2);
        let Stored::Vault(stored) = &entry.stored else {
            panic!("{program:?}: {:?}", entry.stored);
        };
        let run_now = stored.run_now(team.fees.pubkey()).unwrap();
        assert_eq!(run_now.tx.format, TxFormat::V0, "{program:?}");
        assert!(run_now.tx.wire.len() <= 1_232);
        // This chain locks at most 64 accounts a transaction, as a cluster
        // without `increase_tx_account_lock_limit` does, so the node says
        // what the execution, which lists even more, would meet too.
        let simulated = run(simulate_vault_run(&team.node, &run_now)).unwrap();
        let error = simulated.error.expect("more accounts than one locks");
        assert_eq!(error.message, "too many account locks", "{program:?}");
    }
}

/// A multisig of `members` on `desk`'s chain, 2 of 2 unless `threshold`
/// says, three SOL in its vault, acted on by `acting` with the desk's
/// first wallet paying fees.
fn desk_squad(
    desk: &mut Desk,
    program: SquadsProgram,
    members: &[Pubkey],
    threshold: u16,
    acting: WalletId,
) -> Squad {
    let creator = desk.payer.pubkey();
    let setup = SquadsSetup {
        members: members
            .iter()
            .map(|key| SquadsMember {
                key: *key,
                permissions: PERMISSION_ALL,
            })
            .collect(),
        threshold,
        time_lock: 0,
        config_authority: None,
        rent_collector: None,
    };
    let squad = {
        let mut chain = desk.chain();
        let squad = match program {
            SquadsProgram::V4 => {
                let create_key = KeypairSigner::new_random();
                let (squad, create) =
                    Squad::create_v4(&create_key.pubkey(), &creator, &chain.treasury, &setup);
                chain
                    .send(&[&desk.payer, &create_key], vec![create])
                    .unwrap();
                squad
            }
            SquadsProgram::SmartAccount => {
                let config = chain.data(&program.program_config());
                let count = SquadsProgram::smart_account_count(&config).unwrap();
                let (squad, create) =
                    Squad::create_smart_account(count + 1, &creator, &chain.treasury, &setup);
                chain.send(&[&desk.payer], vec![create]).unwrap();
                squad
            }
        };
        chain.fund(&squad.vault(0), 3 * SOL);
        squad
    };
    let mut wallet = WalletRecord::squads("Team", squad, Vec::new(), threshold);
    wallet.kind.set_member_wallet(Some(acting));
    wallet.kind.set_fee_payer(Some(desk.wallets[0].id));
    desk.wallets.push(wallet);
    squad
}

/// What Review and Sign simulates beside a transaction a site asked the
/// vault to sign, once rebuilt as a proposal: `None` when it adds no second
/// simulation.
fn second_simulation(desk: &Desk, wire: &[u8]) -> Option<Simulation> {
    let tx = decode_v1_transaction(wire).unwrap();
    let stored = stored_for_later(&tx);
    let [stored] = stored.as_slice() else {
        assert!(stored.is_empty(), "more than one stored");
        return None;
    };
    let run_now = stored.run_now(tx.fee_payer().unwrap()).unwrap();
    Some(run(simulate_vault_run(&desk.node, &run_now)).unwrap())
}

fn assert_pays(desk: &Desk, simulated: &Simulation, vault: Pubkey, to: Pubkey, label: &str) {
    assert_eq!(simulated.error, None, "{label}: {:?}", simulated.logs);
    let held = desk.chain().balance(&vault);
    assert_eq!(
        simulated.balance_changes,
        vec![change(vault, held, held - SOL), change(to, 0, SOL)],
        "{label}"
    );
}

/// Review and Sign finds what a proposal stores in a plain creation, in one
/// wrapped for a programmatic signer and in one wrapped for a Vector
/// wallet, and adds no second simulation when the same transaction executes
/// it too.
#[test]
fn review_and_sign_simulates_what_a_creation_stores_wherever_it_is_wrapped() {
    for program in BOTH {
        let recipient = KeypairSigner::new_random().pubkey();

        // A hot key proposes; the other member has yet to approve.
        let mut desk = Desk::new();
        let alice = KeypairSigner::new_random();
        desk.chain().fund(&alice.pubkey(), SOL);
        let hot = WalletRecord::from_keypair_seed("Alice", alice.seed_bytes());
        let acting = hot.id;
        desk.wallets.push(hot);
        let other = KeypairSigner::new_random().pubkey();
        let squad = desk_squad(&mut desk, program, &[alice.pubkey(), other], 2, acting);
        let vault = squad.vault(0);
        let pay = solana_system_interface::instruction::transfer(&vault, &recipient, SOL);
        let adapted = desk.rebuild(vault, vec![pay.clone()]);
        let tx = decode_v1_transaction(&adapted.wire).unwrap();
        assert!(tx
            .native_instructions()
            .iter()
            .any(|ix| StoredTransaction::parse(ix).is_some()));
        let simulated = second_simulation(&desk, &adapted.wire).expect("a second simulation");
        assert_pays(
            &desk,
            &simulated,
            vault,
            recipient,
            &format!("{program:?} plain"),
        );

        // 1 of 1: the same transaction executes it.
        let mut desk = Desk::new();
        let alice = KeypairSigner::new_random();
        desk.chain().fund(&alice.pubkey(), SOL);
        let hot = WalletRecord::from_keypair_seed("Alice", alice.seed_bytes());
        let acting = hot.id;
        desk.wallets.push(hot);
        let squad = desk_squad(&mut desk, program, &[alice.pubkey()], 1, acting);
        let vault = squad.vault(0);
        let pay = solana_system_interface::instruction::transfer(&vault, &recipient, SOL);
        let adapted = desk.rebuild(vault, vec![pay]);
        assert_eq!(adapted.steps.len(), 1, "{program:?}: one transaction");
        assert!(
            second_simulation(&desk, &adapted.wire).is_none(),
            "{program:?}: it executes too"
        );

        // A programmatic signer proposes, wrapped in its Submit.
        let mut desk = Desk::new();
        let authority = KeypairSigner::new_random();
        let mut signer = WalletRecord::programmatic("Signer", authority.pubkey(), None);
        let member = signer.address().unwrap();
        {
            let mut chain = desk.chain();
            let fees = desk.payer.pubkey();
            let (nonce, create_nonce) =
                create_executor_nonce(&fees, &fees, "airlock", &member, chain.nonce_rent())
                    .unwrap();
            chain.send(&[&desk.payer], create_nonce.to_vec()).unwrap();
            signer.kind.set_executor_nonce(Some(nonce));
        }
        let acting = signer.id;
        desk.wallets.push(signer);
        let other = KeypairSigner::new_random().pubkey();
        let squad = desk_squad(&mut desk, program, &[member, other], 2, acting);
        let vault = squad.vault(0);
        let pay = solana_system_interface::instruction::transfer(&vault, &recipient, SOL);
        let adapted = desk.rebuild(vault, vec![pay]);
        let tx = decode_v1_transaction(&adapted.wire).unwrap();
        assert!(
            tx.native_instructions()
                .iter()
                .all(|ix| StoredTransaction::parse(ix).is_none()),
            "{program:?}: the creation is inside the Submit"
        );
        let simulated = second_simulation(&desk, &adapted.wire).expect("a second simulation");
        assert_pays(
            &desk,
            &simulated,
            vault,
            recipient,
            &format!("{program:?} programmatic"),
        );

        // A Vector wallet proposes, wrapped in its Passthrough.
        let mut desk = Desk::new();
        let account = desk.add_vector("Quantum", true);
        let other = KeypairSigner::new_random().pubkey();
        let squad = desk_squad(
            &mut desk,
            program,
            &[account.address, other],
            2,
            account.wallet,
        );
        let vault = squad.vault(0);
        let pay = solana_system_interface::instruction::transfer(&vault, &recipient, SOL);
        let adapted = desk.rebuild(vault, vec![pay]);
        let simulated = second_simulation(&desk, &adapted.wire).expect("a second simulation");
        assert_pays(
            &desk,
            &simulated,
            vault,
            recipient,
            &format!("{program:?} Vector"),
        );
    }
}
