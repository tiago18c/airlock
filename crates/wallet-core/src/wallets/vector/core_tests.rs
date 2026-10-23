//! What `wallets/vector` writes out by hand, held to the program's client
//! crate, `vector-core` of our fork, under our program id.

use super::*;
use vector_core::{find_vector_pda, VectorAccount as CoreAccount};

#[test]
fn the_program_id_is_our_deployment() {
    assert_eq!(
        PROGRAM_ID.to_string(),
        "74mXFW7BSAAwc6zQC2uYKuDcWnz17HHXMWqLnEeg9BJv"
    );
}

#[test]
fn every_scheme_has_the_sizes_the_crate_gives_it() {
    assert_eq!(HEADER_LEN, CoreAccount::HEADER_LEN);
    assert_eq!(VECTOR_SEED, vector_core::VECTOR_PDA_SEED);
    assert_eq!(
        PASSTHROUGH_DISCRIMINATOR,
        vector_core::PASSTHROUGH_DISCRIMINATOR
    );
    for scheme in SchemeId::ALL {
        let core = scheme.core();
        assert_eq!(scheme.byte(), core.id, "{scheme:?}");
        assert_eq!(scheme.key_len(), core.identity_len, "{scheme:?}");
        assert_eq!(scheme.signature_len(), core.signature_len, "{scheme:?}");
        assert_eq!(scheme.next_key_len(), core.next_key_len, "{scheme:?}");
        assert_eq!(scheme.account_len(), core.account_len, "{scheme:?}");
    }
}

#[test]
fn the_system_pda_is_the_one_the_crate_derives() {
    let account = address(&PROGRAM_ID, SchemeId::Ed25519, &[7; 32]).unwrap();
    assert_eq!(
        system_pda(&account),
        vector_core::find_system_pda(&account).0
    );
}

#[test]
fn every_address_is_the_one_the_crate_derives() {
    for scheme in SchemeId::ALL {
        let key = vec![7; scheme.key_len()];
        let (expected, _) = find_vector_pda(&scheme.core(), &key);
        assert_eq!(
            address(&PROGRAM_ID, scheme, &key).unwrap(),
            expected,
            "{scheme:?}"
        );
    }
}
