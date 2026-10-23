//! What cannot be rebuilt, and what a rebuild loses.

use super::*;

#[test]
fn what_cannot_be_rebuilt_is_refused_with_the_reason() {
    let mut d = desk();
    let member = KeypairSigner::new_random();
    let member_wallet = WalletRecord::from_keypair_seed("Member", member.seed_bytes());
    let member_id = member_wallet.id;
    d.wallets.push(member_wallet);
    let squad = d.add_squad(SquadsProgram::V4, member_id);
    let vault = squad.vault(0);
    let compiled = |payer: Pubkey, instructions: Vec<Instruction>| {
        let wire = compile_v1(payer, instructions, None, Some(blockhash(1)))
            .unwrap()
            .wire;
        decode_v1_transaction(&wire).unwrap()
    };

    // A second signer inside the vault's instructions.
    let mixed = compiled(
        vault,
        vec![transfer(vault, pk(2), 5), transfer(pk(61), pk(2), 5)],
    );
    assert_eq!(
        plan_adaptation(&mixed, &d.wallets, None),
        Err(AdaptError::OtherSigner {
            squad: "Treasury".into(),
            signer: pk(61),
        })
    );
    // A fee payer the instructions never use is simply replaced.
    let sponsored = compiled(pk(61), vec![transfer(vault, pk(2), 5)]);
    assert!(plan_adaptation(&sponsored, &d.wallets, None)
        .unwrap()
        .is_some());

    // Two multisigs in one transaction.
    let other = Squad::v4(&pk(51));
    let mut second = WalletRecord::squads("Second", other, Vec::new(), 1);
    if let WalletKind::Squads { member_wallet, .. } = &mut second.kind {
        *member_wallet = Some(member_id);
    }
    let mut two = d.wallets.clone();
    two.push(second);
    let both = compiled(
        vault,
        vec![
            transfer(vault, pk(2), 5),
            transfer(other.vault(0), pk(2), 5),
        ],
    );
    assert_eq!(
        plan_adaptation(&both, &two, None),
        Err(AdaptError::SeveralMultisigs)
    );

    // A durable nonce, which a proposal cannot advance.
    let wire = compile_v1(
        vault,
        vec![transfer(vault, pk(2), 5)],
        Some(NonceSpec {
            account: pk(3),
            authority: vault,
            hash: blockhash(4),
        }),
        None,
    )
    .unwrap()
    .wire;
    assert_eq!(
        plan_adaptation(&decode_v1_transaction(&wire).unwrap(), &d.wallets, None),
        Err(AdaptError::DurableNonce("Treasury".into()))
    );

    // The chain disagrees with what the vault stores.
    let tx = site_tx(vault);
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    assert_eq!(
        plan.build(&tx, &[], blockhash(4)),
        Err(AdaptError::NotOnChain("Treasury".into()))
    );
    let (address, mut account) =
        squad_account(&squad, 1, 0, 0, &[(member.pubkey(), PERMISSION_ALL)]);
    account.owner = pk(1);
    assert_eq!(
        plan.build(&tx, &[(address, account)], blockhash(4)),
        Err(AdaptError::Unreadable("Treasury".into()))
    );
    let strangers = [squad_account(&squad, 1, 0, 0, &[(pk(8), PERMISSION_ALL)])];
    assert_eq!(
        plan.build(&tx, &strangers, blockhash(4)),
        Err(AdaptError::NotAMember {
            squad: "Treasury".into(),
            member: member.pubkey(),
        })
    );

    // A transaction that already carries an authorization.
    let direct = site_tx(d.address);
    let plan = plan_adaptation(&direct, &d.wallets, None).unwrap().unwrap();
    let wrapped = plan
        .build(&direct, &[d.nonce_account()], blockhash(4))
        .unwrap();
    assert_eq!(
        plan_adaptation(
            &decode_v1_transaction(&wrapped.wire).unwrap(),
            &d.wallets,
            None
        ),
        Ok(None)
    );
    // A nonce another address controls would need that address to sign.
    let (nonce, mut account) = d.nonce_account();
    account.data[32..64].copy_from_slice(pk(9).as_ref());
    assert_eq!(
        plan.build(&direct, &[(nonce, account)], blockhash(4)),
        Err(AdaptError::ForeignNonce("Signer".into()))
    );
    assert_eq!(
        plan.build(&direct, &[], blockhash(4)),
        Err(AdaptError::NotOnChain("The nonce account of Signer".into()))
    );
}

#[test]
fn signatures_the_site_added_are_counted_as_lost() {
    let d = desk();
    let extra = KeypairSigner::new_random();
    let wire = compile_v1(
        d.address,
        vec![
            transfer(d.address, pk(2), 5),
            transfer(extra.pubkey(), pk(2), 5),
        ],
        None,
        Some(blockhash(1)),
    )
    .unwrap()
    .wire;
    let wire = apply_local_signatures(&wire, &[&extra as &dyn WalletSigner]).unwrap();
    let tx = decode_v1_transaction(&wire).unwrap();
    let plan = plan_adaptation(&tx, &d.wallets, None).unwrap().unwrap();
    let adapted = plan.build(&tx, &[d.nonce_account()], blockhash(4)).unwrap();
    assert_eq!(adapted.voided, 1);
    // That key still has to sign, now the transaction around the
    // authorization, and it is not on this device.
    let rebuilt = decode_v1_transaction(&adapted.wire).unwrap();
    assert_eq!(
        signer_keys(&rebuilt),
        vec![d.payer.pubkey(), extra.pubkey()]
    );
    assert!(rebuilt.signers.iter().all(|slot| !slot.filled));
    assert_eq!(
        programmatic_signer_address(&d.authority.pubkey()),
        d.address
    );
}

#[test]
fn a_vector_wallet_of_another_deployment_is_refused_as_signer_and_as_member() {
    use crate::wallets::record::Authority;
    use crate::wallets::vector::SchemeId;
    let mut d = desk();
    let mut vector = WalletRecord::vector(
        "Quantum",
        SchemeId::Ed25519,
        pk(70),
        d.authority.pubkey().to_bytes().to_vec(),
        Authority::Outside,
    );
    vector.kind.set_fee_payer(Some(d.payer_id));
    let (account, vector_id) = (vector.address().unwrap(), vector.id);
    d.wallets.push(vector);
    let compiled = |payer: Pubkey, instructions: Vec<Instruction>| {
        let wire = compile_v1(payer, instructions, None, Some(blockhash(1)))
            .unwrap()
            .wire;
        decode_v1_transaction(&wire).unwrap()
    };
    // Made under a program this build does not sign for.
    let refused = Err(AdaptError::OtherProgram("Quantum".into()));

    // Asked to sign as itself.
    let own = compiled(d.payer.pubkey(), vec![transfer(account, pk(2), 5)]);
    assert_eq!(plan_adaptation(&own, &d.wallets, None), refused);
    // Asked to act as the member of a multisig: the proposal is not built
    // for a member nothing could then sign as.
    let squad = d.add_squad(SquadsProgram::V4, vector_id);
    let vault = squad.vault(0);
    let through = compiled(vault, vec![transfer(vault, pk(2), 5)]);
    assert_eq!(plan_adaptation(&through, &d.wallets, None), refused);
    // A transaction that does not name it is left alone.
    let plain = compiled(d.payer.pubkey(), vec![transfer(d.payer.pubkey(), pk(2), 5)]);
    assert_eq!(plan_adaptation(&plain, &d.wallets, None), Ok(None));
}
