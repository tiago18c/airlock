//! The reference values here were printed by the program's own client crate,
//! `vector-core` of `blueshift-gg/vector` at `094fa5a` (branch
//! `feat/single-program`), under the program id in that source. Our fork
//! compiles another id in, so the values are written out. `core_tests` holds
//! the same functions to the fork's crate under our id.

use super::*;
use crate::chain::RawAccount;
use crate::fixtures::pk;
use solana_instruction::{AccountMeta, Instruction};
use std::str::FromStr;

/// The program id in upstream's source. Nothing is deployed there.
const UPSTREAM: &str = "vectorcLBXJ2TuoKuUygkEi6FWqvBnbHDEDWoYamfjV";
const SYSVAR: &str = "Sysvar1nstructions1111111111111111111111111";

fn program() -> Pubkey {
    Pubkey::from_str(UPSTREAM).unwrap()
}

fn key_of(scheme: SchemeId) -> Vec<u8> {
    vec![7; scheme.key_len()]
}

/// A complete account of `scheme`, as `Initialize` leaves it.
fn account(scheme: SchemeId, key: &[u8]) -> RawAccount {
    let mut data = vec![0u8; scheme.account_len()];
    data[..32].copy_from_slice(&[5; 32]);
    data[32..64].copy_from_slice(&seed(key));
    data[64] = scheme.byte();
    data[65] = 254;
    data[66] = 253;
    // Every scheme but Falcon-512 keeps the key first.
    if scheme != SchemeId::Falcon512 {
        data[HEADER_LEN..HEADER_LEN + key.len()].copy_from_slice(key);
    }
    RawAccount {
        lamports: 1,
        data,
        owner: program(),
        executable: false,
        rent_epoch: 0,
    }
}

#[test]
fn every_scheme_has_the_sizes_and_the_address_the_program_gives_it() {
    let reference = [
        (
            SchemeId::Ed25519,
            1,
            64,
            32,
            100,
            "5pcSenKB8hXe5W7JEMhoQVuFCDHHWNYqVDewJZfQXSvY",
        ),
        (
            SchemeId::Eip191,
            2,
            65,
            20,
            88,
            "C5vjXSBxeDC5tNt7dC74RiredJo7aPGEpbNzwtFvdM74",
        ),
        (
            SchemeId::Secp256k1,
            3,
            64,
            33,
            101,
            "8ZP9DgiVXxAtP3kxEPddN6JtJqvgWPAoeYC3UXFCLwds",
        ),
        (
            SchemeId::Falcon512,
            4,
            666,
            897,
            1092,
            "EU4PpxJB2izzE4BDCGqAfuSHsiKuf9kVJaHJm3z2fFeb",
        ),
        (
            SchemeId::MlDsa44,
            5,
            2420,
            1312,
            21_924,
            "G5Kv1LRgwebHUL1KZxGDvt8MTMmJVWpisYxE9EReuDBK",
        ),
        (
            SchemeId::Winternitz,
            6,
            849,
            41,
            109,
            "ADptnFTgjs5n5t1bPGBM5Nb6QqM9uYGXbTvMuZiE1iW8",
        ),
        (
            SchemeId::Xmss,
            7,
            1037,
            41,
            113,
            "2dELfUWuxRg4NJrn7fsiTLwL3JC1doP47Qv8eRDTxZQp",
        ),
    ];
    assert_eq!(reference.len(), SchemeId::ALL.len());
    for (scheme, byte, signature, key, account, at) in reference {
        assert_eq!(scheme.byte(), byte, "{scheme:?}");
        assert_eq!(SchemeId::from_byte(byte), Some(scheme));
        assert_eq!(scheme.signature_len(), signature, "{scheme:?}");
        assert_eq!(scheme.key_len(), key, "{scheme:?}");
        assert_eq!(scheme.account_len(), account, "{scheme:?}");
        assert_eq!(
            address(&program(), scheme, &key_of(scheme))
                .unwrap()
                .to_string(),
            at,
            "{scheme:?}"
        );
    }
    assert_eq!(SchemeId::from_byte(0), None);
}

#[test]
fn an_address_belongs_to_one_key_one_scheme_and_one_program() {
    let key = [7u8; 32];
    let here = address(&program(), SchemeId::Ed25519, &key).unwrap();
    assert_ne!(
        here,
        address(&program(), SchemeId::Ed25519, &[8; 32]).unwrap()
    );
    assert_ne!(here, address(&pk(1), SchemeId::Ed25519, &key).unwrap());
    // A key of the wrong length is refused, not hashed into some address.
    assert_eq!(
        address(&program(), SchemeId::Secp256k1, &key),
        Err(VectorError::KeyLength {
            scheme: "secp256k1",
            expected: 33,
            found: 32
        })
    );
}

#[test]
fn an_account_is_read_for_its_nonce_its_key_and_its_next_leaf() {
    for scheme in SchemeId::ALL {
        let key = key_of(scheme);
        let read = VectorAccount::parse(&program(), &account(scheme, &key)).unwrap();
        assert_eq!(read.scheme, scheme);
        assert_eq!(read.nonce, [5; 32]);
        assert!(read.created_with(&key), "{scheme:?}");
        assert!(!read.created_with(&[9; 32]), "{scheme:?}");
        match scheme {
            SchemeId::Falcon512 => assert_eq!(read.current_key(), None),
            _ => assert_eq!(read.current_key(), Some(key.as_slice()), "{scheme:?}"),
        }
        assert_eq!(read.next_leaf().is_some(), scheme == SchemeId::Xmss);
    }

    // XMSS: the key that signs now is not the one that named the account,
    // and the chain says which leaf may sign next.
    let first = key_of(SchemeId::Xmss);
    let mut moved = account(SchemeId::Xmss, &first);
    moved.data[HEADER_LEN..HEADER_LEN + 41].copy_from_slice(&[4; 41]);
    moved.data[HEADER_LEN + 41..].copy_from_slice(&300u32.to_le_bytes());
    let read = VectorAccount::parse(&program(), &moved).unwrap();
    assert!(read.created_with(&first));
    assert_eq!(read.current_key(), Some([4u8; 41].as_slice()));
    assert_eq!(read.next_leaf(), Some(300));
}

#[test]
fn an_account_that_is_not_a_complete_vector_account_is_refused_with_the_reason() {
    let key = [7u8; 32];
    let good = account(SchemeId::Ed25519, &key);
    let parse = |account: &RawAccount| VectorAccount::parse(&program(), account).unwrap_err();

    let mut foreign = good.clone();
    foreign.owner = pk(3);
    assert_eq!(
        parse(&foreign),
        VectorError::ForeignOwner {
            owner: pk(3),
            program: program()
        }
    );
    let mut short = good.clone();
    short.data.truncate(HEADER_LEN - 1);
    assert_eq!(parse(&short), VectorError::Truncated);
    // An account anyone assigned to the program has zeroed data: scheme 0
    // belongs to none.
    let mut zeroed = good.clone();
    zeroed.data.fill(0);
    assert_eq!(parse(&zeroed), VectorError::UnsupportedScheme(0));
    let mut winternitz = good.clone();
    winternitz.data[64] = 6;
    assert_eq!(
        parse(&winternitz),
        VectorError::Incomplete {
            scheme: "Winternitz",
            expected: SchemeId::Winternitz.account_len(),
            found: good.data.len()
        }
    );

    // An ML-DSA-44 account is created 10,240 bytes long and grown twice.
    let mut growing = account(SchemeId::MlDsa44, &key_of(SchemeId::MlDsa44));
    growing.data.truncate(10_240);
    assert_eq!(
        parse(&growing),
        VectorError::Incomplete {
            scheme: "ML-DSA-44",
            expected: 21_924,
            found: 10_240
        }
    );
}

fn meta(pubkey: Pubkey, is_signer: bool, is_writable: bool) -> AccountMeta {
    AccountMeta {
        pubkey,
        is_signer,
        is_writable,
    }
}

fn hex(text: &str) -> Vec<u8> {
    hex::decode(text).unwrap()
}

/// What upstream builds for two instructions run as the Ed25519 account of
/// `[7; 32]`: one it signs and pays from, one it co-signs with another key.
fn reference_passthrough() -> (Pubkey, Instruction) {
    let vector = address(&program(), SchemeId::Ed25519, &[7; 32]).unwrap();
    let instruction = Instruction {
        program_id: program(),
        accounts: vec![
            meta(Pubkey::from_str(SYSVAR).unwrap(), false, false),
            meta(vector, false, false),
            meta(pk(9), false, false),
            meta(vector, false, true),
            meta(pk(2), false, true),
            meta(pk(8), false, false),
            meta(pk(3), true, false),
            meta(vector, false, false),
        ],
        // One signer, as itself; two instructions.
        data: hex("01010002020300010203020000"),
    };
    (vector, instruction)
}

#[test]
fn a_passthrough_is_read_back_into_what_it_runs() {
    let (vector, instruction) = reference_passthrough();
    let crate_built = vector_core::create_passthrough_instruction(
        &SchemeId::Ed25519.core(),
        &[7; 32],
        &[
            Instruction {
                program_id: pk(9),
                accounts: vec![meta(vector, true, true), meta(pk(2), false, true)],
                data: vec![1, 2, 3],
            },
            Instruction {
                program_id: pk(8),
                accounts: vec![meta(pk(3), true, false), meta(vector, true, false)],
                data: vec![],
            },
        ],
    );
    // The crate builds under our program id, so its addresses differ; the
    // layout of the data does not.
    assert_eq!(crate_built.data, instruction.data);

    let read = parse_passthrough(&instruction).unwrap();
    assert_eq!(
        read.signers,
        vec![PassthroughSigner {
            account: vector,
            system: false
        }]
    );
    // The account signs wherever it appears, as the program makes it. The
    // other signer and each writable flag are as the instruction has them.
    assert_eq!(
        read.instructions,
        vec![
            Instruction {
                program_id: pk(9),
                accounts: vec![meta(vector, true, true), meta(pk(2), false, true)],
                data: vec![1, 2, 3],
            },
            Instruction {
                program_id: pk(8),
                accounts: vec![meta(pk(3), true, false), meta(vector, true, false)],
                data: vec![],
            },
        ]
    );
}

#[test]
fn a_passthrough_reads_the_same_whatever_the_scheme_of_its_account() {
    let runs = vec![Instruction {
        program_id: pk(9),
        accounts: vec![meta(pk(2), false, true)],
        data: vec![5],
    }];
    for scheme in SchemeId::ALL {
        let key = vec![7; scheme.key_len()];
        let built = passthrough(scheme, &key, &runs).unwrap();
        assert_eq!(built.data, hex("0101000101010005"), "{scheme:?}");
        assert_eq!(parse_passthrough(&built).unwrap().instructions, runs);
    }
}

#[test]
fn a_passthrough_that_would_not_run_is_not_read() {
    let (_, good) = reference_passthrough();
    let read = |change: &dyn Fn(&mut Instruction)| {
        let mut instruction = good.clone();
        change(&mut instruction);
        parse_passthrough(&instruction).unwrap_err()
    };
    let malformed = VectorError::MalformedPassthrough;

    assert_eq!(read(&|ix| ix.data[0] = 0), VectorError::NotPassthrough);
    assert_eq!(read(&|ix| ix.data.clear()), VectorError::NotPassthrough);
    assert_eq!(
        read(&|ix| ix.accounts.clear()),
        malformed("it lists no accounts")
    );
    assert_eq!(
        read(&|ix| ix.data[1] = 0),
        malformed("it names no account to run as")
    );
    assert_eq!(
        read(&|ix| ix.data[2] = 2),
        malformed("a signer is neither the account nor its system PDA")
    );
    assert_eq!(read(&|ix| ix.data.truncate(8)), malformed("it ends early"));
    assert_eq!(
        read(&|ix| ix.data.push(0)),
        malformed("bytes follow its last instruction")
    );
    assert_eq!(
        read(&|ix| ix.accounts.push(meta(pk(4), false, false))),
        malformed("it lists accounts no instruction uses")
    );
    assert_eq!(
        read(&|ix| {
            ix.accounts.pop();
        }),
        malformed("it lists fewer accounts than its instructions use")
    );
}

#[test]
fn only_the_hash_based_schemes_and_falcon_depend_on_the_build() {
    for scheme in SchemeId::ALL {
        let expected = match scheme {
            SchemeId::Falcon512 | SchemeId::Winternitz | SchemeId::Xmss => {
                !cfg!(target_arch = "wasm32")
            }
            _ => true,
        };
        assert_eq!(signs_here(scheme), expected, "{}", scheme.label());
        assert!(scheme_offered(scheme), "{}", scheme.label());
    }
}

#[test]
fn a_key_is_shown_the_way_its_scheme_writes_it() {
    let ed25519 = pk(9);
    assert_eq!(
        SchemeId::Ed25519.show_key(&ed25519.to_bytes()),
        ed25519.to_string()
    );
    assert_eq!(
        SchemeId::Eip191.show_key(&[0xab; 20]),
        format!("0x{}", "ab".repeat(20))
    );
    assert_eq!(SchemeId::Secp256k1.show_key(&[2; 33]), "02".repeat(33));
    let long = key_of(SchemeId::MlDsa44);
    let shown = SchemeId::MlDsa44.show_key(&long);
    assert_eq!(
        shown,
        format!("1312-byte key, hash {}…", hex::encode(&seed(&long)[..8]))
    );
}
