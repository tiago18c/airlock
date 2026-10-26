//! Site requests rebuilt as far as fits after the Vector wrap: the ladder
//! of shapes, co-voters from the row, and measured sizes per scheme.

use crate::support::vector::{run, Desk, SecpAccount};
use crate::support::{Chain, SOL};
use wallet_core::adapt::{AdaptedStep, HeldBack};
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::proposals::{chosen_members, proposal_shapes};
use wallet_core::squads::{Squad, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL};
use wallet_core::vector::keys_to_sign_with;
use wallet_core::{
    decode_v1_transaction, KeypairSigner, MemoryStore, Vault, WalletId, WalletRecord, WalletSigner,
};
use wallet_flows::sign::{Signed, VectorSigning};

const BOTH: [SquadsProgram; 2] = [SquadsProgram::V4, SquadsProgram::SmartAccount];

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

fn squad_wallet(desk: &mut Desk, squad: Squad, acting: WalletId, with: Vec<WalletId>) -> WalletId {
    let mut wallet = WalletRecord::squads("Treasury", squad, Vec::new(), 2);
    wallet.kind.set_member_wallet(Some(acting));
    wallet.kind.set_approves_with(with);
    wallet.kind.set_fee_payer(Some(desk.wallets[0].id));
    let id = wallet.id;
    desk.wallets.push(wallet);
    id
}

fn member_of(address: solana_pubkey::Pubkey) -> SquadsMember {
    SquadsMember {
        key: address,
        permissions: PERMISSION_ALL,
    }
}

fn transfer(
    vault: &solana_pubkey::Pubkey,
    to: &solana_pubkey::Pubkey,
) -> solana_instruction::Instruction {
    solana_system_interface::instruction::transfer(vault, to, SOL)
}

/// Sign as [`Desk::sign`], plus any extra hot keys that also sign the transaction.
fn sign_with_keys(
    desk: &mut Desk,
    wire: &[u8],
    known: &[wallet_core::relay::VectorContext],
    extra: &[&dyn WalletSigner],
) -> Result<Signed, String> {
    let known = match decode_v1_transaction(wire) {
        Ok(tx) => run(wallet_flows::vector::read_context(&desk.node, &tx, known))
            .unwrap_or_else(|_| known.to_vec()),
        Err(_) => known.to_vec(),
    };
    let mut vault = Vault::new(MemoryStore::new());
    vault.create("desk").unwrap();
    vault.data_mut().unwrap().wallets = desk.wallets.clone();
    let mut local: Vec<&dyn WalletSigner> = vec![&desk.payer];
    local.extend_from_slice(extra);
    let keys = keys_to_sign_with(
        &mut vault,
        &mut desk.leaf_marks,
        &desk.xmss_retries,
        wire,
        &local,
        &known,
    )
    .map_err(|err| err.to_string())?;
    desk.wallets = vault.data().unwrap().wallets.clone();
    let mut vector = VectorSigning {
        keys: &keys,
        known: &known,
        cache: Some(&mut desk.xmss_retries),
        known_is_current: false,
    };
    crate::support::sign_through(Some(&desk.node), wire, &local, &mut vector)
        .map_err(|err| err.to_string())
}

#[test]
fn two_ed25519_vector_members_execute_a_site_request_together() {
    for program in BOTH {
        let mut desk = Desk::new();
        let first = desk.add_vector("First", true);
        let second = desk.add_vector("Second", true);
        let squad = {
            let mut chain = desk.chain();
            create_squad(
                &mut chain,
                program,
                &desk.payer,
                vec![member_of(first.address), member_of(second.address)],
                2,
            )
        };
        squad_wallet(&mut desk, squad, first.wallet, vec![second.wallet]);
        let vault = squad.vault(0);
        let recipient = KeypairSigner::new_random().pubkey();
        let adapted = desk.rebuild(vault, vec![transfer(&vault, &recipient)]);
        assert!(
            matches!(
                &adapted.steps[0],
                AdaptedStep::Squads {
                    voters,
                    executor: Some(_),
                    held_back: None,
                    ..
                } if voters.len() == 2
            ),
            "{program:?}: {:?}",
            adapted.steps[0]
        );
        let signed = desk
            .sign(&adapted.wire, &adapted.context)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        let landed = desk.land(&signed.wire);
        println!(
            "{program:?} 2-of-2 Ed25519 site request: {} CU, {} bytes",
            landed.compute_units, landed.size
        );
        assert_eq!(desk.chain().balance(&recipient), SOL, "{program:?}");
    }
}

#[test]
fn a_hot_key_and_an_mldsa_member_execute_a_site_request_together() {
    for program in BOTH {
        let mut desk = Desk::new();
        let hot = KeypairSigner::new_random();
        desk.chain().fund(&hot.pubkey(), SOL);
        let hot_wallet = WalletRecord::from_keypair_seed("Hot", hot.seed_bytes());
        let hot_id = hot_wallet.id;
        desk.wallets.push(hot_wallet);
        let quantum = desk.add_mldsa("Dilithium");
        let squad = {
            let mut chain = desk.chain();
            create_squad(
                &mut chain,
                program,
                &desk.payer,
                vec![member_of(hot.pubkey()), member_of(quantum.address)],
                2,
            )
        };
        squad_wallet(&mut desk, squad, hot_id, vec![quantum.wallet]);
        let vault = squad.vault(0);
        let recipient = KeypairSigner::new_random().pubkey();
        let adapted = desk.rebuild(vault, vec![transfer(&vault, &recipient)]);
        assert!(
            matches!(
                &adapted.steps[0],
                AdaptedStep::Squads {
                    voters,
                    executor: Some(_),
                    held_back: None,
                    ..
                } if voters.len() == 2
            ),
            "{program:?}: {:?}",
            adapted.steps[0]
        );
        let signed = sign_with_keys(&mut desk, &adapted.wire, &adapted.context, &[&hot])
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        let landed = desk.land(&signed.wire);
        println!(
            "{program:?} hot + ML-DSA site request: {} CU, {} bytes",
            landed.compute_units, landed.size
        );
        assert_eq!(desk.chain().balance(&recipient), SOL, "{program:?}");
    }
}

#[test]
fn two_mldsa_members_leave_the_second_vote_and_execute_out() {
    for program in BOTH {
        let mut desk = Desk::new();
        let first = desk.add_mldsa("First");
        let second = desk.add_mldsa("Second");
        let squad = {
            let mut chain = desk.chain();
            create_squad(
                &mut chain,
                program,
                &desk.payer,
                vec![member_of(first.address), member_of(second.address)],
                2,
            )
        };
        squad_wallet(&mut desk, squad, first.wallet, vec![second.wallet]);
        let vault = squad.vault(0);
        let recipient = KeypairSigner::new_random().pubkey();
        let adapted = desk.rebuild(vault, vec![transfer(&vault, &recipient)]);
        assert!(
            matches!(
                &adapted.steps[0],
                AdaptedStep::Squads {
                    voters,
                    executor: None,
                    held_back: Some(HeldBack::Size { .. }),
                    ..
                } if voters.len() == 1 && voters[0] == first.address
            ),
            "{program:?}: {:?}",
            adapted.steps[0]
        );
        let signed = desk
            .sign(&adapted.wire, &adapted.context)
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        let landed = desk.land(&signed.wire);
        println!(
            "{program:?} 2-of-2 ML-DSA left approved: {} CU, {} bytes",
            landed.compute_units, landed.size
        );
        assert_eq!(desk.chain().balance(&recipient), 0, "{program:?}");
    }
}

#[test]
fn a_programmatic_and_a_vector_co_voter_execute_together() {
    for program in BOTH {
        let mut desk = Desk::new();
        let vector = desk.add_vector("Quantum", true);
        let authority = KeypairSigner::new_random();
        let mut signer = WalletRecord::programmatic("Signer", authority.pubkey(), None);
        let signer_address = signer.address().unwrap();
        let (nonce_account, create_nonce) = create_executor_nonce(
            &desk.payer.pubkey(),
            &desk.payer.pubkey(),
            "airlock",
            &signer_address,
            desk.chain().nonce_rent(),
        )
        .unwrap();
        desk.chain()
            .send(&[&desk.payer], create_nonce.to_vec())
            .unwrap();
        signer.kind.set_executor_nonce(Some(nonce_account));
        signer.kind.set_fee_payer(Some(desk.wallets[0].id));
        let signer_id = signer.id;
        desk.wallets.push(signer);
        let squad = {
            let mut chain = desk.chain();
            create_squad(
                &mut chain,
                program,
                &desk.payer,
                vec![member_of(vector.address), member_of(signer_address)],
                2,
            )
        };
        let multisig = squad_wallet(&mut desk, squad, vector.wallet, vec![signer_id]);
        let state = squad
            .parse_state(&desk.chain().data(&squad.address))
            .unwrap();
        let chosen = chosen_members(&desk.wallets, multisig, &state);
        let shapes = proposal_shapes(&state, &[], &chosen);
        assert!(
            matches!(
                shapes.first(),
                Some(shape) if shape.voters.len() == 2 && shape.executor.is_some()
            ),
            "{program:?}: {shapes:?}"
        );

        let vault = squad.vault(0);
        let recipient = KeypairSigner::new_random().pubkey();
        let adapted = desk.rebuild(vault, vec![transfer(&vault, &recipient)]);
        assert!(
            matches!(
                &adapted.steps[0],
                AdaptedStep::Squads {
                    voters,
                    executor: Some(_),
                    held_back: None,
                    ..
                } if voters.len() == 2
            ),
            "{program:?}: {:?}",
            adapted.steps[0]
        );
        let signed = sign_with_keys(&mut desk, &adapted.wire, &adapted.context, &[&authority])
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        let landed = desk.land(&signed.wire);
        println!(
            "{program:?} programmatic + Vector site request: {} CU, {} bytes",
            landed.compute_units, landed.size
        );
        assert_eq!(desk.chain().balance(&recipient), SOL, "{program:?}");
    }
}

/// The programmatic member creates. Were it to execute too, the Vector vote
/// would have to run inside its `Submit`, after its execute. The Vector
/// member executes instead.
#[test]
fn a_programmatic_creator_leaves_execute_to_its_vector_co_voter() {
    for program in BOTH {
        let mut desk = Desk::new();
        let vector = desk.add_vector("Quantum", true);
        let authority = KeypairSigner::new_random();
        let mut signer = WalletRecord::programmatic("Signer", authority.pubkey(), None);
        let signer_address = signer.address().unwrap();
        let (nonce_account, create_nonce) = create_executor_nonce(
            &desk.payer.pubkey(),
            &desk.payer.pubkey(),
            "airlock",
            &signer_address,
            desk.chain().nonce_rent(),
        )
        .unwrap();
        desk.chain()
            .send(&[&desk.payer], create_nonce.to_vec())
            .unwrap();
        signer.kind.set_executor_nonce(Some(nonce_account));
        signer.kind.set_fee_payer(Some(desk.wallets[0].id));
        let signer_id = signer.id;
        desk.wallets.push(signer);
        let squad = {
            let mut chain = desk.chain();
            create_squad(
                &mut chain,
                program,
                &desk.payer,
                vec![member_of(vector.address), member_of(signer_address)],
                2,
            )
        };
        let multisig = squad_wallet(&mut desk, squad, signer_id, vec![vector.wallet]);
        let state = squad
            .parse_state(&desk.chain().data(&squad.address))
            .unwrap();
        let chosen = chosen_members(&desk.wallets, multisig, &state);
        let shapes = proposal_shapes(&state, &[], &chosen);
        assert!(
            matches!(
                shapes.first(),
                Some(shape) if shape.voters.len() == 2 && shape.executor.is_some()
            ),
            "{program:?}: {shapes:?}"
        );

        let vault = squad.vault(0);
        let recipient = KeypairSigner::new_random().pubkey();
        let adapted = desk.rebuild(vault, vec![transfer(&vault, &recipient)]);
        assert!(
            matches!(
                &adapted.steps[0],
                AdaptedStep::Squads {
                    voters,
                    executor: Some(executor),
                    held_back: None,
                    ..
                } if *voters == vec![signer_address, vector.address]
                    && *executor == vector.address
            ),
            "{program:?}: {:?}",
            adapted.steps[0]
        );
        let signed = sign_with_keys(&mut desk, &adapted.wire, &adapted.context, &[&authority])
            .unwrap_or_else(|err| panic!("{program:?}: {err}"));
        let landed = desk.land(&signed.wire);
        println!(
            "{program:?} programmatic creator, Vector co-voter executes: {} CU, {} bytes",
            landed.compute_units, landed.size
        );
        assert_eq!(desk.chain().balance(&recipient), SOL, "{program:?}");
    }
}

type AddScheme = fn(&mut Desk, &str) -> SecpAccount;

/// Bytes and compute units for one System transfer, per scheme × shape.
#[test]
fn scheme_by_shape_sizes_for_one_system_transfer() {
    let program = SquadsProgram::V4;
    let schemes: &[(&str, AddScheme)] = &[
        ("ed25519", |d, l| {
            let a = d.add_vector(l, true);
            SecpAccount {
                address: a.address,
                wallet: a.wallet,
            }
        }),
        ("secp256k1", Desk::add_secp),
        ("eip191", Desk::add_eip191),
        ("mldsa44", Desk::add_mldsa),
        ("falcon512", Desk::add_falcon),
        ("xmss", Desk::add_xmss),
        ("winternitz", Desk::add_winternitz),
    ];
    for (name, add) in schemes {
        let mut desk = Desk::new();
        let account = add(&mut desk, "Member");
        let squad = {
            let mut chain = desk.chain();
            create_squad(
                &mut chain,
                program,
                &desk.payer,
                vec![member_of(account.address)],
                1,
            )
        };
        squad_wallet(&mut desk, squad, account.wallet, Vec::new());
        let vault = squad.vault(0);
        let recipient = KeypairSigner::new_random().pubkey();
        let adapted = desk.rebuild(vault, vec![transfer(&vault, &recipient)]);
        let (voters, executor, held) = match &adapted.steps[0] {
            AdaptedStep::Squads {
                voters,
                executor,
                held_back,
                ..
            } => (voters.len(), executor.is_some(), held_back.clone()),
            other => panic!("{name}: {other:?}"),
        };
        let tx = decode_v1_transaction(&adapted.wire).unwrap();
        let cu = tx.budget.compute_unit_limit.unwrap_or(0);
        println!(
            "{name}: {} bytes, {cu} CU, voters={voters}, execute={executor}, held_back={held:?}",
            adapted.wire.len()
        );
        if executor {
            let signed = desk
                .sign(&adapted.wire, &adapted.context)
                .unwrap_or_else(|err| panic!("{name}: {err}"));
            let landed = desk.land(&signed.wire);
            println!(
                "{name} landed: {} bytes, {} CU",
                landed.size, landed.compute_units
            );
        }
    }
}
