//! A programmatic signer and a Vector wallet share one rebuilt transaction.
//! Full execute, a size-held partial, and a complete refusal are covered; so
//! is signing order (programmatic before Vector).

use crate::support::vector::{run, Desk};
use crate::support::{sign_through, Chain, SOL};
use wallet_core::adapt::{plan_adaptation, AdaptedStep, HeldBack};
use wallet_core::programmatic::create_executor_nonce;
use wallet_core::squads::{Squad, SquadsMember, SquadsProgram, SquadsSetup, PERMISSION_ALL};
use wallet_core::vector::keys_to_sign_with;
use wallet_core::{
    decode_v1_transaction, KeypairSigner, MemoryStore, Vault, WalletId, WalletRecord, WalletSigner,
};
use wallet_flows::compile::compile_fresh;
use wallet_flows::sign::{sign_with, VectorSigning};

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

fn member_of(key: solana_pubkey::Pubkey) -> SquadsMember {
    SquadsMember {
        key,
        permissions: PERMISSION_ALL,
    }
}

fn add_programmatic(
    desk: &mut Desk,
    label: &str,
) -> (KeypairSigner, WalletId, solana_pubkey::Pubkey) {
    let authority = KeypairSigner::new_random();
    let mut signer = WalletRecord::programmatic(label, authority.pubkey(), None);
    let address = signer.address().unwrap();
    // A seed per signer, so two of them do not ask for one account.
    let seed = format!("airlock-{label}");
    let (nonce, create) = create_executor_nonce(
        &desk.payer.pubkey(),
        &desk.payer.pubkey(),
        &seed,
        &address,
        desk.chain().nonce_rent(),
    )
    .unwrap();
    desk.chain().send(&[&desk.payer], create.to_vec()).unwrap();
    signer.kind.set_executor_nonce(Some(nonce));
    signer.kind.set_fee_payer(Some(desk.wallets[0].id));
    let id = signer.id;
    desk.wallets.push(signer);
    (authority, id, address)
}

fn sign_mixed(
    desk: &mut Desk,
    wire: &[u8],
    known: &[wallet_core::relay::VectorContext],
    authority: &KeypairSigner,
) -> Vec<u8> {
    sign_mixed_by(desk, wire, known, &[authority])
}

fn sign_mixed_by(
    desk: &mut Desk,
    wire: &[u8],
    known: &[wallet_core::relay::VectorContext],
    authorities: &[&KeypairSigner],
) -> Vec<u8> {
    let known = match decode_v1_transaction(wire) {
        Ok(tx) => run(wallet_flows::vector::read_context(&desk.node, &tx, known))
            .unwrap_or_else(|_| known.to_vec()),
        Err(_) => known.to_vec(),
    };
    let mut vault = Vault::new(MemoryStore::new());
    vault.create("desk").unwrap();
    vault.data_mut().unwrap().wallets = desk.wallets.clone();
    let local: Vec<&dyn WalletSigner> = std::iter::once(&desk.payer as &dyn WalletSigner)
        .chain(authorities.iter().map(|key| *key as &dyn WalletSigner))
        .collect();
    let keys = keys_to_sign_with(
        &mut vault,
        &mut desk.leaf_marks,
        &desk.xmss_retries,
        wire,
        &local,
        &known,
    )
    .unwrap();
    desk.wallets = vault.data().unwrap().wallets.clone();
    let mut vector = VectorSigning {
        keys: &keys,
        known: &known,
        cache: Some(&mut desk.xmss_retries),
        known_is_current: false,
    };
    sign_through(Some(&desk.node), wire, &local, &mut vector)
        .unwrap()
        .wire
}

fn transfer(
    vault: &solana_pubkey::Pubkey,
    to: &solana_pubkey::Pubkey,
) -> solana_instruction::Instruction {
    solana_system_interface::instruction::transfer(vault, to, SOL)
}

/// Full build: one System transfer fits create, both votes and execute.
#[test]
fn a_site_request_with_programmatic_and_vector_members_executes() {
    for program in BOTH {
        let mut desk = Desk::new();
        let vector = desk.add_vector("Quantum", true);
        let (authority, signer_id, signer_address) = add_programmatic(&mut desk, "Signer");
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
        squad_wallet(&mut desk, squad, vector.wallet, vec![signer_id]);
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
        assert!(
            adapted
                .steps
                .iter()
                .any(|step| matches!(step, AdaptedStep::Programmatic { .. }))
                && adapted
                    .steps
                    .iter()
                    .any(|step| matches!(step, AdaptedStep::Vector { .. })),
            "{program:?}: {:?}",
            adapted.steps
        );
        let wire = sign_mixed(&mut desk, &adapted.wire, &adapted.context, &authority);
        let landed = desk.land(&wire);
        println!(
            "{program:?} mixed full execute: {} CU, {} bytes",
            landed.compute_units, landed.size
        );
        assert_eq!(desk.chain().balance(&recipient), SOL, "{program:?}");
    }
}

/// Partial: ML-DSA + programmatic makes the fullest shape miss, so execute
/// (and possibly a vote) is held back.
#[test]
fn a_large_mixed_shape_is_left_partial() {
    for program in BOTH {
        let mut desk = Desk::new();
        let vector = desk.add_mldsa("Dilithium");
        let (authority, signer_id, signer_address) = add_programmatic(&mut desk, "Signer");
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
        squad_wallet(&mut desk, squad, signer_id, vec![vector.wallet]);
        let vault = squad.vault(0);
        let payments: Vec<_> = (0..6u8)
            .map(|_| {
                solana_system_interface::instruction::transfer(
                    &vault,
                    &KeypairSigner::new_random().pubkey(),
                    1,
                )
            })
            .collect();
        let adapted = desk.rebuild(vault, payments);
        assert!(
            matches!(
                &adapted.steps[0],
                AdaptedStep::Squads {
                    executor: None,
                    held_back: Some(HeldBack::Size { .. } | HeldBack::Compute { .. }),
                    ..
                }
            ),
            "{program:?}: {:?}",
            adapted.steps[0]
        );
        let wire = sign_mixed(&mut desk, &adapted.wire, &adapted.context, &authority);
        let landed = desk.land(&wire);
        println!(
            "{program:?} mixed left partial: {} CU, {} bytes",
            landed.compute_units, landed.size
        );
        assert!(
            landed.size <= wallet_core::MAX_TRANSACTION_SIZE,
            "{program:?}"
        );
    }
}

/// Complete failure: even create and propose do not fit behind ML-DSA.
#[test]
fn a_mixed_row_refuses_when_even_create_and_propose_do_not_fit() {
    for program in BOTH {
        let mut desk = Desk::new();
        let vector = desk.add_mldsa("Dilithium");
        let (_authority, signer_id, signer_address) = add_programmatic(&mut desk, "Signer");
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
        squad_wallet(&mut desk, squad, vector.wallet, vec![signer_id]);
        let vault = squad.vault(0);
        let payments: Vec<_> = (0..40u8)
            .map(|_| {
                solana_system_interface::instruction::transfer(
                    &vault,
                    &KeypairSigner::new_random().pubkey(),
                    1,
                )
            })
            .collect();
        let (wire, _) = run(compile_fresh(&desk.node, vault, payments)).unwrap();
        let tx = decode_v1_transaction(&wire).unwrap();
        let plan = plan_adaptation(&tx, &desk.wallets, None)
            .unwrap()
            .expect("the vault is asked to sign");
        let mut accounts = Vec::new();
        for address in plan.accounts_to_read() {
            if let Some(account) = desk.chain().account(&address) {
                accounts.push((address, account));
            }
        }
        let error = plan
            .build(&tx, &accounts, desk.chain().latest_blockhash())
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("too large") && message.contains("bytes"),
            "{program:?}: {message}"
        );
    }
}

/// The Vector digest covers the `Submit`. Signing Vector first is refused
/// until the programmatic authority has signed.
#[test]
fn the_vector_key_waits_for_the_programmatic_authority() {
    let mut desk = Desk::new();
    let vector = desk.add_vector("Quantum", true);
    let (authority, signer_id, signer_address) = add_programmatic(&mut desk, "Signer");
    let squad = {
        let mut chain = desk.chain();
        create_squad(
            &mut chain,
            SquadsProgram::V4,
            &desk.payer,
            vec![member_of(vector.address), member_of(signer_address)],
            2,
        )
    };
    squad_wallet(&mut desk, squad, vector.wallet, vec![signer_id]);
    let vault = squad.vault(0);
    let recipient = KeypairSigner::new_random().pubkey();
    let adapted = desk.rebuild(vault, vec![transfer(&vault, &recipient)]);

    // Vector is held until Submit is signed: nothing is signable with only
    // the fee payer and the Vector key.
    let tx = decode_v1_transaction(&adapted.wire).unwrap();
    let status = wallet_core::signing_status_in(
        &tx,
        &wallet_core::LocalKeys {
            keys: &[desk.payer.pubkey(), vector.key.pubkey()],
            vector: &[vector.address],
        },
        &wallet_core::relay::AuthContext {
            vector: &adapted.context,
        },
    );
    assert_eq!(
        status.signable, 0,
        "Vector must wait for Submit: {status:?}"
    );
    assert!(status.waiting_for_authority || status.authorizations_elsewhere() > 0);

    let wire = sign_mixed(&mut desk, &adapted.wire, &adapted.context, &authority);
    let landed = desk.land(&wire);
    println!(
        "mixed after ordered signing: {} CU, {} bytes",
        landed.compute_units, landed.size
    );
    assert_eq!(desk.chain().balance(&recipient), SOL);
}

/// Two programmatic and two Vector members, listed mixed on the row: the
/// approvals are reordered so one `Submit` carries both programmatic
/// members' instructions and one `Passthrough` runs as both Vector accounts.
#[test]
fn two_programmatic_and_two_vector_members_share_one_submit_and_one_passthrough() {
    let mut desk = Desk::new();
    let (first, first_id, first_address) = add_programmatic(&mut desk, "First");
    let v1 = desk.add_vector("Quantum", true);
    let (second, second_id, second_address) = add_programmatic(&mut desk, "Second");
    let v2 = desk.add_vector("Photon", true);
    let squad = {
        let mut chain = desk.chain();
        create_squad(
            &mut chain,
            SquadsProgram::V4,
            &desk.payer,
            vec![
                member_of(first_address),
                member_of(v1.address),
                member_of(second_address),
                member_of(v2.address),
            ],
            4,
        )
    };
    // Listed mixed, so the build has to reorder the approvals.
    squad_wallet(
        &mut desk,
        squad,
        first_id,
        vec![v1.wallet, second_id, v2.wallet],
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
            } if voters.len() == 4
        ),
        "{:?}",
        adapted.steps[0]
    );
    let tx = decode_v1_transaction(&adapted.wire).unwrap();
    let programs: Vec<_> = tx
        .native_instructions()
        .iter()
        .map(|instruction| instruction.program_id)
        .collect();
    assert_eq!(
        programs,
        vec![
            wallet_core::vector::PROGRAM_ID,
            wallet_core::programmatic::ED25519_SIGNER_PROGRAM_ID,
            wallet_core::vector::PROGRAM_ID,
        ]
    );
    let passthrough = wallet_core::vector::parse_passthrough(&tx.native_instructions()[2]).unwrap();
    let runs_as: Vec<_> = passthrough
        .signers
        .iter()
        .map(|signer| signer.account)
        .collect();
    assert_eq!(runs_as.len(), 2);
    assert!(runs_as.contains(&v1.address) && runs_as.contains(&v2.address));
    let authorities = wallet_core::relay::authority_slots(&tx);
    assert_eq!(authorities.len(), 4, "{authorities:?}");

    let wire = sign_mixed_by(
        &mut desk,
        &adapted.wire,
        &adapted.context,
        &[&first, &second],
    );
    let landed = desk.land(&wire);
    println!(
        "two programmatic + two Vector: {} CU, {} bytes",
        landed.compute_units, landed.size
    );
    assert_eq!(desk.chain().balance(&recipient), SOL);
}

/// Signed as the offline key device does: from the note only, no node.
fn sign_offline(
    desk: &mut Desk,
    wire: &[u8],
    context: &[wallet_core::relay::VectorContext],
    authorities: &[&KeypairSigner],
) -> Result<Vec<u8>, String> {
    let known = crate::support::vector::note(context);
    let mut vault = Vault::new(MemoryStore::new());
    vault.create("desk").unwrap();
    vault.data_mut().unwrap().wallets = desk.wallets.clone();
    let local: Vec<&dyn WalletSigner> = std::iter::once(&desk.payer as &dyn WalletSigner)
        .chain(authorities.iter().map(|key| *key as &dyn WalletSigner))
        .collect();
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
    sign_through(None, wire, &local, &mut vector)
        .map(|signed| signed.wire)
        .map_err(|err| err.to_string())
}

/// The online device signs the programmatic `Submit`, and an offline device
/// with only the note signs for a Falcon and an XMSS member; or that device
/// holds the authority too, and signs all three authorizations in one press. Filling the
/// `Submit` clears Vector signatures, and has to keep the next key the XMSS
/// account names to keep its key: before and after a transfer and a
/// rotation.
#[test]
fn an_xmss_and_a_falcon_member_sign_after_the_programmatic_authority_offline() {
    for (before, same_press) in [
        ("none", false),
        ("transfer", false),
        ("rotate", false),
        ("none", true),
        ("rotate", true),
    ] {
        let mut desk = Desk::new();
        let (authority, signer_id, signer_address) = add_programmatic(&mut desk, "Signer");
        let falcon = desk.add_falcon("Falcon");
        let xmss = desk.add_xmss("Tree");
        let squad = {
            let mut chain = desk.chain();
            create_squad(
                &mut chain,
                SquadsProgram::V4,
                &desk.payer,
                vec![
                    member_of(signer_address),
                    member_of(falcon.address),
                    member_of(xmss.address),
                ],
                3,
            )
        };
        squad_wallet(
            &mut desk,
            squad,
            signer_id,
            vec![falcon.wallet, xmss.wallet],
        );
        let receiver = KeypairSigner::new_random().pubkey();
        match before {
            "transfer" | "rotate" => {
                let adapted = desk.rebuild(
                    xmss.address,
                    vec![solana_system_interface::instruction::transfer(
                        &xmss.address,
                        &receiver,
                        SOL / 8,
                    )],
                );
                let wire = sign_offline(&mut desk, &adapted.wire, &adapted.context, &[]).unwrap();
                desk.land(&wire);
            }
            _ => {}
        }
        if before == "rotate" {
            let rotation = desk.sign_rotation(xmss.wallet);
            let first_key = desk
                .wallets
                .iter()
                .find(|w| w.id == xmss.wallet)
                .and_then(|w| w.kind.vector_settings())
                .unwrap()
                .key
                .to_vec();
            let prepared = run(wallet_flows::vector::rotate(
                &desk.node,
                desk.payer.pubkey(),
                wallet_core::vector::SchemeId::Xmss,
                &first_key,
                &rotation,
            ))
            .unwrap();
            let paid = desk.sign(&prepared.wire, &[]).unwrap();
            desk.land(&paid.wire);
        }
        let vault = squad.vault(0);
        let adapted = desk.rebuild(vault, vec![transfer(&vault, &receiver)]);
        let tx = decode_v1_transaction(&adapted.wire).unwrap();
        let programs: Vec<_> = tx
            .native_instructions()
            .iter()
            .map(|instruction| instruction.program_id)
            .collect();
        assert_eq!(
            programs,
            vec![
                wallet_core::vector::PROGRAM_ID,
                wallet_core::programmatic::ED25519_SIGNER_PROGRAM_ID,
                wallet_core::vector::PROGRAM_ID,
            ],
            "{before}"
        );
        if same_press {
            // The authority is on the device that holds both Vector keys,
            // and all three authorizations sign in one press.
            let wire = sign_offline(&mut desk, &adapted.wire, &adapted.context, &[&authority])
                .unwrap_or_else(|err| panic!("{before}, one press: {err}"));
            desk.land(&wire);
            assert!(desk.chain().balance(&receiver) >= SOL, "{before}");
            continue;
        }
        // The online device signs the `Submit`, then hands the transaction
        // and its note to the device that holds both Vector keys.
        let online = run(sign_with(
            Some(&desk.node),
            &adapted.wire,
            &[&authority as &dyn WalletSigner],
            &mut VectorSigning {
                keys: &[],
                known: &adapted.context,
                cache: None,
                known_is_current: false,
            },
        ))
        .unwrap();
        assert_eq!(online.authorizations, 1, "{before}");
        let wire = sign_offline(&mut desk, &online.wire, &adapted.context, &[])
            .unwrap_or_else(|err| panic!("{before}: {err}"));
        desk.land(&wire);
        assert!(desk.chain().balance(&receiver) >= SOL, "{before}");
    }
}
