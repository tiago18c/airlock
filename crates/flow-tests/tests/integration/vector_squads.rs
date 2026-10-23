//! Vector wallets as members of a Squads multisig, against the real
//! programs: a proposal made by one, and two approving together.

use crate::support::vector::{run, Account, Desk};
use crate::support::{Chain, SOL};
use solana_pubkey::Pubkey;
use wallet_core::proposals::{chosen_members, combined_approval, combined_instructions};
use wallet_core::squads::{
    Squad, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL, PERMISSION_INITIATE,
};
use wallet_core::{decode_v1_transaction, KeypairSigner, WalletId, WalletRecord, WalletSigner};
use wallet_flows::compile::compile_fresh;

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

/// A multisig of `members`, created by `creator`, whose vault holds three
/// SOL.
fn create_squad(
    chain: &mut Chain,
    program: SquadsProgram,
    creator: &KeypairSigner,
    members: Vec<SquadsMember>,
    threshold: u16,
) -> Squad {
    let setup = SquadsSetup {
        members,
        threshold,
        time_lock: 0,
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

/// The multisig as a wallet of this device, acted on by `acting`.
fn squad_wallet(desk: &mut Desk, squad: Squad, acting: WalletId, with: Vec<WalletId>) -> WalletId {
    let mut wallet = WalletRecord::squads("Treasury", squad, Vec::new(), 2);
    wallet.kind.set_member_wallet(Some(acting));
    wallet.kind.set_approves_with(with);
    wallet.kind.set_fee_payer(Some(desk.wallets[0].id));
    let id = wallet.id;
    desk.wallets.push(wallet);
    id
}

fn member(account: &Account) -> SquadsMember {
    SquadsMember {
        key: account.address,
        permissions: PERMISSION_ALL,
    }
}

#[test]
fn a_vector_wallet_proposes_approves_and_executes_for_its_multisig() {
    for program in BOTH {
        let mut desk = Desk::new();
        let account = desk.add_vector("Quantum", true);
        let squad = {
            let mut chain = desk.chain();
            create_squad(&mut chain, program, &desk.payer, vec![member(&account)], 1)
        };
        squad_wallet(&mut desk, squad, account.wallet, Vec::new());
        let vault = squad.vault(0);
        let recipient = KeypairSigner::new_random().pubkey();

        // The site asked the vault to pay, and to pay the fee.
        let adapted = desk.rebuild(
            vault,
            vec![solana_system_interface::instruction::transfer(
                &vault, &recipient, SOL,
            )],
        );
        assert_eq!(adapted.steps.len(), 2, "{program:?}");
        let signed = desk
            .sign(&adapted.wire, &adapted.context)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        let landed = desk.land(&signed.wire);
        println!(
            "{program:?} proposal by a Vector member: {} CU, {} bytes",
            landed.compute_units, landed.size
        );
        let chain = desk.chain();
        assert_eq!(chain.balance(&recipient), SOL, "{program:?}");
        assert_eq!(chain.balance(&vault), 2 * SOL, "{program:?}");
    }
}

#[test]
fn two_vector_members_approve_and_execute_in_one_transaction() {
    for program in BOTH {
        let mut desk = Desk::new();
        let first = desk.add_vector("First", true);
        let second = desk.add_vector("Second", true);
        let creator = KeypairSigner::new_random();
        desk.chain().fund(&creator.pubkey(), 10 * SOL);
        let squad = {
            let mut chain = desk.chain();
            let members = vec![
                member(&first),
                member(&second),
                SquadsMember {
                    key: creator.pubkey(),
                    permissions: PERMISSION_INITIATE,
                },
            ];
            create_squad(&mut chain, program, &creator, members, 2)
        };
        let multisig = squad_wallet(&mut desk, squad, first.wallet, vec![second.wallet]);

        // Somebody else stored the payment and opened its proposal.
        let vault = squad.vault(0);
        let recipient = KeypairSigner::new_random().pubkey();
        let payment = solana_system_interface::instruction::transfer(&vault, &recipient, SOL);
        let by = creator.pubkey();
        desk.chain()
            .send(
                &[&creator],
                vec![
                    squad
                        .create_transaction(1, 0, std::slice::from_ref(&payment), &by, &by)
                        .unwrap(),
                    squad.create_proposal(1, &by, &by),
                ],
            )
            .unwrap_or_else(|failure| panic!("{program:?}: {}", crate::support::logs(&failure)));

        // What the Proposals modal offers, compiled and rebuilt on Sign.
        let (state, entries) =
            run(wallet_flows::squads::load_page(&desk.node, &squad, None)).unwrap();
        let entry = entries.into_iter().find(|entry| entry.index == 1).unwrap();
        let chosen = chosen_members(&desk.wallets, multisig, &state);
        let combined = combined_approval(&state, &entry, &chosen, None)
            .expect("the two Vector members can finish the proposal together");
        assert_eq!(combined.voters, vec![first.address, second.address]);
        let instructions = combined_instructions(&squad, &entry, &combined).unwrap();
        let (built, _) = run(compile_fresh(&desk.node, desk.payer.pubkey(), instructions)).unwrap();
        let adapted = desk.rebuild_wire(&built);
        let tx = decode_v1_transaction(&adapted.wire).unwrap();
        let signers: Vec<Pubkey> = tx.signers.iter().map(|slot| slot.pubkey).collect();
        assert_eq!(signers, vec![desk.payer.pubkey()], "{program:?}");

        let signed = desk
            .sign(&adapted.wire, &adapted.context)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        assert_eq!(signed.authorizations, 2, "{program:?}");
        let landed = desk.land(&signed.wire);
        println!(
            "{program:?} combined approval by two Vector members: {} CU, {} bytes",
            landed.compute_units, landed.size
        );
        assert_eq!(desk.chain().balance(&recipient), SOL, "{program:?}");
    }
}
