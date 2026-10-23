//! The checked builders: each refuses what the crate's builder would panic
//! on, and a `Passthrough` built here reads back into what it was built from.

use super::*;
use crate::fixtures::pk;
use solana_instruction::{AccountMeta, Instruction};

fn key_of(scheme: SchemeId) -> Vec<u8> {
    vec![7; scheme.key_len()]
}

fn transfer(from: Pubkey, to: Pubkey) -> Instruction {
    Instruction {
        program_id: crate::ids::SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(from, true), AccountMeta::new(to, false)],
        data: vec![2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
    }
}

fn with_accounts(count: usize) -> Instruction {
    Instruction {
        program_id: pk(9),
        accounts: (0..count)
            .map(|i| AccountMeta::new_readonly(pk(u8::try_from(i % 200).unwrap()), false))
            .collect(),
        data: vec![],
    }
}

/// An account of an `Advance` that has not signed yet.
fn entry<'a>(
    scheme: SchemeId,
    key: &'a [u8],
    next_key: &'a [u8],
    signature: &'a [u8],
) -> AdvanceEntry<'a> {
    AdvanceEntry {
        scheme,
        key,
        next_key,
        signature,
    }
}

#[test]
fn a_key_of_the_wrong_length_is_refused_by_every_builder() {
    let short = [7u8; 31];
    let wrong = VectorError::KeyLength {
        scheme: "Ed25519",
        expected: 32,
        found: 31,
    };
    let ed = SchemeId::Ed25519;
    assert_eq!(initialize(&pk(1), ed, &short).unwrap_err(), wrong);
    assert_eq!(passthrough(ed, &short, &[]).unwrap_err(), wrong);
    assert_eq!(withdraw(ed, &short, &pk(1), 1).unwrap_err(), wrong);
    assert_eq!(close(ed, &short, &pk(1)).unwrap_err(), wrong);
    assert_eq!(
        advance(&[entry(ed, &short, &[], &[0; 64])]).unwrap_err(),
        wrong
    );
}

#[test]
fn a_signature_of_the_wrong_length_is_refused() {
    let key = key_of(SchemeId::Eip191);
    assert_eq!(
        advance(&[entry(SchemeId::Eip191, &key, &[], &[0; 64])]).unwrap_err(),
        VectorError::SignatureLength {
            scheme: "EIP-191",
            expected: 65,
            found: 64
        }
    );
}

#[test]
fn an_ml_dsa_account_is_created_in_three_initializes() {
    for scheme in SchemeId::ALL {
        let key = key_of(scheme);
        let account = address(&PROGRAM_ID, scheme, &key).unwrap();
        let made = initialize(&pk(1), scheme, &key).unwrap();
        let calls = if scheme == SchemeId::MlDsa44 { 3 } else { 1 };
        assert_eq!(made.len(), calls, "{scheme:?}");
        // The first takes the scheme and the key; each later one grows the
        // same account.
        assert_eq!(made[0].data[1], scheme.byte(), "{scheme:?}");
        assert_eq!(&made[0].data[2..], key.as_slice(), "{scheme:?}");
        for grow in &made[1..] {
            assert_eq!(grow.data, vec![made[0].data[0]]);
            assert_eq!(grow.accounts[1].pubkey, account);
        }
    }
}

#[test]
fn an_advance_lists_each_account_once_and_fits_one_instruction() {
    assert_eq!(advance(&[]).unwrap_err(), VectorError::NoSigners);

    let key = key_of(SchemeId::Ed25519);
    let twice = advance(&[
        entry(SchemeId::Ed25519, &key, &[], &[0; 64]),
        entry(SchemeId::Ed25519, &key, &[], &[0; 64]),
    ]);
    let account = address(&PROGRAM_ID, SchemeId::Ed25519, &key).unwrap();
    assert_eq!(twice.unwrap_err(), VectorError::SignerTwice(account));

    // The same bytes under two schemes are two accounts.
    let both = advance(&[
        entry(SchemeId::Ed25519, &[7; 32], &[], &[0; 64]),
        entry(SchemeId::Secp256k1, &[7; 33], &[], &[0; 64]),
    ])
    .unwrap();
    assert_eq!(both.accounts.len(), 3);
    assert_eq!(both.data.len(), 1 + 64 + 64);

    let keys: Vec<[u8; 32]> = (0..=254u8).map(|i| [i; 32]).collect();
    let signers: Vec<_> = keys
        .iter()
        .map(|key| entry(SchemeId::Ed25519, key, &[], &[0; 64]))
        .collect();
    assert_eq!(
        advance(&signers).unwrap_err(),
        VectorError::TooManySigners(255)
    );
    assert_eq!(advance(&signers[..254]).unwrap().accounts.len(), 255);
}

#[test]
fn an_advance_names_the_next_key_of_a_rotating_account_before_its_signature() {
    let ed = key_of(SchemeId::Ed25519);
    assert_eq!(
        advance(&[entry(SchemeId::Ed25519, &ed, &[1; 32], &[0; 64])]).unwrap_err(),
        VectorError::KeepsItsKey { scheme: "Ed25519" }
    );
    let xmss = key_of(SchemeId::Xmss);
    let xmss_signature = vec![0; SchemeId::Xmss.signature_len()];
    assert_eq!(
        advance(&[entry(SchemeId::Xmss, &xmss, &[1; 40], &xmss_signature)]).unwrap_err(),
        VectorError::KeyLength {
            scheme: "XMSS",
            expected: 41,
            found: 40
        }
    );
    let wots = key_of(SchemeId::Winternitz);
    let wots_signature = vec![9; SchemeId::Winternitz.signature_len()];
    assert_eq!(
        advance(&[entry(SchemeId::Winternitz, &wots, &[], &wots_signature)]).unwrap_err(),
        VectorError::NeedsNextKey {
            scheme: "Winternitz"
        }
    );

    let next = [4u8; 41];
    let built = advance(&[
        entry(SchemeId::Ed25519, &ed, &[], &[8; 64]),
        entry(SchemeId::Winternitz, &wots, &next, &wots_signature),
    ])
    .unwrap();
    let mut expected = vec![ADVANCE_DISCRIMINATOR];
    expected.extend([8; 64]);
    expected.extend(next);
    expected.extend(&wots_signature);
    assert_eq!(built.data, expected);
}

#[test]
fn a_passthrough_holds_what_one_instruction_can_hold() {
    let scheme = SchemeId::Ed25519;
    let key = key_of(scheme);
    let build = |instructions: &[Instruction]| passthrough(scheme, &key, instructions);

    let many = vec![with_accounts(0); 256];
    assert_eq!(
        build(&many).unwrap_err(),
        VectorError::TooManyInstructions(256)
    );
    assert_eq!(
        build(&[with_accounts(1), with_accounts(256)]).unwrap_err(),
        VectorError::InnerAccounts {
            index: 1,
            found: 256
        }
    );
    let mut long = with_accounts(0);
    long.data = vec![0; 65_536];
    assert_eq!(
        build(&[long]).unwrap_err(),
        VectorError::InnerData {
            index: 0,
            found: 65_536
        }
    );
    // Two in front, then a program and its accounts: 2 + 1 + 252 is the most.
    assert_eq!(build(&[with_accounts(252)]).unwrap().accounts.len(), 255);
    assert_eq!(
        build(&[with_accounts(253)]).unwrap_err(),
        VectorError::PassthroughAccounts(256)
    );
    assert_eq!(
        build(&[with_accounts(100), with_accounts(100), with_accounts(51)]).unwrap_err(),
        VectorError::PassthroughAccounts(256)
    );
}

#[test]
fn a_passthrough_built_here_reads_back_into_what_it_was_built_from() {
    for scheme in [
        SchemeId::Ed25519,
        SchemeId::MlDsa44,
        SchemeId::Xmss,
        SchemeId::Winternitz,
    ] {
        let key = key_of(scheme);
        let account = address(&PROGRAM_ID, scheme, &key).unwrap();
        let inner = vec![
            transfer(account, pk(2)),
            withdraw(scheme, &key, &pk(3), 5).unwrap(),
            Instruction {
                program_id: pk(4),
                accounts: vec![AccountMeta::new_readonly(pk(5), false)],
                data: vec![1, 2, 3],
            },
        ];
        let built = passthrough(scheme, &key, &inner).unwrap();
        assert_eq!(built.program_id, PROGRAM_ID);
        // The program grants the account its signature inside the call.
        assert!(built.accounts.iter().all(|meta| !meta.is_signer));

        let read = parse_passthrough(&built).unwrap();
        assert_eq!(
            read.signers,
            vec![PassthroughSigner {
                account,
                system: false
            }],
            "{scheme:?}"
        );
        // Withdraw names the account without a signer flag; reading it back
        // gives it one, as the program does.
        let mut expected = inner.clone();
        expected[1].accounts[0].is_signer = true;
        assert_eq!(read.instructions, expected, "{scheme:?}");
    }
}

#[test]
fn a_passthrough_of_several_signers_reads_who_signs_as_what() {
    let ed = key_of(SchemeId::Ed25519);
    let secp = key_of(SchemeId::Secp256k1);
    let ed_core = SchemeId::Ed25519.core();
    let secp_core = SchemeId::Secp256k1.core();
    let ed_account = address(&PROGRAM_ID, SchemeId::Ed25519, &ed).unwrap();
    let secp_account = address(&PROGRAM_ID, SchemeId::Secp256k1, &secp).unwrap();
    let system = system_pda(&secp_account);
    let inner = vec![transfer(system, pk(2)), transfer(ed_account, pk(3))];
    let built = vector_core::create_multi_passthrough_instruction(
        &[
            vector_core::PassthroughSigner::vector(&ed_core, &ed),
            vector_core::PassthroughSigner::system(&secp_core, &secp),
        ],
        &inner,
    );

    let read = parse_passthrough(&built).unwrap();
    assert_eq!(
        read.signers,
        vec![
            PassthroughSigner {
                account: ed_account,
                system: false
            },
            PassthroughSigner {
                account: secp_account,
                system: true
            },
        ]
    );
    assert_eq!(read.signers[1].address(), system);
    assert_eq!(read.instructions, inner);
}

#[test]
fn a_passthrough_that_is_not_what_the_program_runs_is_refused() {
    let key = key_of(SchemeId::Ed25519);
    let built = passthrough(SchemeId::Ed25519, &key, &[with_accounts(1)]).unwrap();

    let mut none = built.clone();
    none.data[1] = 0;
    assert!(matches!(
        parse_passthrough(&none),
        Err(VectorError::MalformedPassthrough(_))
    ));
    let mut odd = built.clone();
    odd.data[2] = 2;
    assert!(matches!(
        parse_passthrough(&odd),
        Err(VectorError::MalformedPassthrough(_))
    ));
    let mut trailing = built.clone();
    trailing.data.push(0);
    assert!(matches!(
        parse_passthrough(&trailing),
        Err(VectorError::MalformedPassthrough(_))
    ));
    let mut extra = built;
    extra.accounts.push(AccountMeta::new_readonly(pk(1), false));
    assert!(matches!(
        parse_passthrough(&extra),
        Err(VectorError::MalformedPassthrough(_))
    ));
}
