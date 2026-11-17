//! The checked builders: each refuses what the crate's builder would panic
//! on, and a `Passthrough` built here reads back into what it was built from.

use super::*;
use crate::fixtures::pk;
use solana_instruction::{AccountMeta, Instruction};

fn key_of(scheme: SchemeId) -> Vec<u8> {
    vec![7; scheme.key_len()]
}

/// The Ed25519 base point, a key of large order in its canonical encoding.
const ED25519_BASE: [u8; 32] = {
    let mut key = [0x66; 32];
    key[0] = 0x58;
    key
};

/// The secp256k1 generator, compressed.
const SECP256K1_GENERATOR: [u8; 33] = [
    0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b,
    0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17,
    0x98,
];

/// A key of `scheme` the program creates an account for.
fn creatable_key(scheme: SchemeId) -> Vec<u8> {
    match scheme {
        SchemeId::Ed25519 => ED25519_BASE.to_vec(),
        SchemeId::Secp256k1 => SECP256K1_GENERATOR.to_vec(),
        _ => key_of(scheme),
    }
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
        advance(u64::MAX, &[entry(ed, &short, &[], &[0; 64])]).unwrap_err(),
        wrong
    );
}

#[test]
fn a_signature_of_the_wrong_length_is_refused() {
    let key = key_of(SchemeId::Eip191);
    assert_eq!(
        advance(u64::MAX, &[entry(SchemeId::Eip191, &key, &[], &[0; 64])]).unwrap_err(),
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
        let key = creatable_key(scheme);
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
    assert_eq!(advance(u64::MAX, &[]).unwrap_err(), VectorError::NoSigners);

    let key = key_of(SchemeId::Ed25519);
    let twice = advance(
        u64::MAX,
        &[
            entry(SchemeId::Ed25519, &key, &[], &[0; 64]),
            entry(SchemeId::Ed25519, &key, &[], &[0; 64]),
        ],
    );
    let account = address(&PROGRAM_ID, SchemeId::Ed25519, &key).unwrap();
    assert_eq!(twice.unwrap_err(), VectorError::SignerTwice(account));

    // The same bytes under two schemes are two accounts.
    let both = advance(
        u64::MAX,
        &[
            entry(SchemeId::Ed25519, &[7; 32], &[], &[0; 64]),
            entry(SchemeId::Secp256k1, &[7; 33], &[], &[0; 64]),
        ],
    )
    .unwrap();
    assert_eq!(both.accounts.len(), 3);
    assert_eq!(both.data.len(), ADVANCE_HEADER_LEN + 64 + 64);

    let keys: Vec<[u8; 32]> = (0..=254u8).map(|i| [i; 32]).collect();
    let signers: Vec<_> = keys
        .iter()
        .map(|key| entry(SchemeId::Ed25519, key, &[], &[0; 64]))
        .collect();
    assert_eq!(
        advance(u64::MAX, &signers).unwrap_err(),
        VectorError::TooManySigners(255)
    );
    assert_eq!(
        advance(u64::MAX, &signers[..254]).unwrap().accounts.len(),
        255
    );
}

#[test]
fn an_advance_names_the_next_key_of_a_rotating_account_before_its_signature() {
    let ed = key_of(SchemeId::Ed25519);
    assert_eq!(
        advance(
            u64::MAX,
            &[entry(SchemeId::Ed25519, &ed, &[1; 32], &[0; 64])]
        )
        .unwrap_err(),
        VectorError::KeepsItsKey { scheme: "Ed25519" }
    );
    let xmss = key_of(SchemeId::Xmss);
    let xmss_signature = vec![0; SchemeId::Xmss.signature_len()];
    assert_eq!(
        advance(
            u64::MAX,
            &[entry(SchemeId::Xmss, &xmss, &[1; 40], &xmss_signature)]
        )
        .unwrap_err(),
        VectorError::KeyLength {
            scheme: "XMSS",
            expected: 41,
            found: 40
        }
    );
    let wots = key_of(SchemeId::Winternitz);
    let wots_signature = vec![9; SchemeId::Winternitz.signature_len()];
    assert_eq!(
        advance(
            u64::MAX,
            &[entry(SchemeId::Winternitz, &wots, &[], &wots_signature)]
        )
        .unwrap_err(),
        VectorError::NeedsNextKey {
            scheme: "Winternitz"
        }
    );

    let next = [4u8; 41];
    let built = advance(
        1_234,
        &[
            entry(SchemeId::Ed25519, &ed, &[], &[8; 64]),
            entry(SchemeId::Winternitz, &wots, &next, &wots_signature),
        ],
    )
    .unwrap();
    let mut expected = vec![ADVANCE_DISCRIMINATOR];
    expected.extend(1_234u64.to_le_bytes());
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
        let wallet = system_pda(&account);
        let inner = vec![
            transfer(wallet, pk(2)),
            // The account itself, named without a signer flag: the
            // `Passthrough` signs as the wallet, not as the account.
            withdraw(scheme, &key, &pk(3), 5).unwrap(),
            Instruction {
                program_id: pk(4),
                accounts: vec![AccountMeta::new_readonly(pk(5), false)],
                data: vec![1, 2, 3],
            },
        ];
        let built = passthrough(scheme, &key, &inner).unwrap();
        assert_eq!(built.program_id, PROGRAM_ID);
        // The program grants the wallet its signature inside the call.
        assert!(built.accounts.iter().all(|meta| !meta.is_signer));

        let read = parse_passthrough(&built).unwrap();
        assert_eq!(
            read.signers,
            vec![PassthroughSigner {
                account,
                system: true
            }],
            "{scheme:?}"
        );
        assert_eq!(read.signers[0].address(), wallet);
        assert_eq!(read.instructions, inner, "{scheme:?}");
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

#[test]
fn a_key_the_program_refuses_gets_no_account() {
    let refused = |scheme: SchemeId| VectorError::KeyRefused {
        scheme: scheme.label(),
    };
    let ed = SchemeId::Ed25519;
    assert!(initialize(&pk(1), ed, &ED25519_BASE).is_ok());
    // A point of small order: the identity, and the point of order 2.
    let mut identity = [0u8; 32];
    identity[0] = 1;
    assert_eq!(initialize(&pk(1), ed, &identity).unwrap_err(), refused(ed));
    let mut order_two = [0xff; 32];
    order_two[0] = 0xec;
    order_two[31] = 0x7f;
    assert_eq!(initialize(&pk(1), ed, &order_two).unwrap_err(), refused(ed));
    // A second encoding of the identity point: y = p + 1.
    let mut unreduced = [0xff; 32];
    unreduced[0] = 0xee;
    unreduced[31] = 0x7f;
    assert_eq!(initialize(&pk(1), ed, &unreduced).unwrap_err(), refused(ed));
    // Bytes that are no point at all.
    let off_curve = (0..=255u8)
        .map(|byte| [byte; 32])
        .find(|key| !Pubkey::new_from_array(*key).is_on_curve())
        .unwrap();
    assert_eq!(initialize(&pk(1), ed, &off_curve).unwrap_err(), refused(ed));

    let secp = SchemeId::Secp256k1;
    assert!(initialize(&pk(1), secp, &SECP256K1_GENERATOR).is_ok());
    let mut prefix = SECP256K1_GENERATOR;
    prefix[0] = 0x04;
    assert_eq!(
        initialize(&pk(1), secp, &prefix).unwrap_err(),
        refused(secp)
    );
    // x = 5 lifts to no point of secp256k1.
    let mut no_point = [0u8; 33];
    no_point[0] = 0x02;
    no_point[32] = 5;
    assert_eq!(
        initialize(&pk(1), secp, &no_point).unwrap_err(),
        refused(secp)
    );

    let eip = SchemeId::Eip191;
    assert_eq!(initialize(&pk(1), eip, &[0; 20]).unwrap_err(), refused(eip));

    // A key pasted in is held to the same rules.
    assert_eq!(
        parse_public_key(ed, &Pubkey::new_from_array(identity).to_string()).unwrap_err(),
        refused(ed)
    );
}
