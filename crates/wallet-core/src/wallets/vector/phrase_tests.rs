use super::*;
use sha2::{Digest, Sha256};

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

#[test]
fn a_phrase_makes_the_same_keys_every_time_and_pins_them() {
    let first = phrase_seed(PHRASE, SchemeId::Xmss, 0).unwrap();
    let second = phrase_seed(PHRASE, SchemeId::Xmss, 1).unwrap();
    assert_ne!(first, second);
    assert_eq!(first, phrase_seed(PHRASE, SchemeId::Xmss, 0).unwrap());
    // Only a scheme whose key moves on is made from a phrase, and each has
    // its own magic string.
    assert!(phrase_seed(PHRASE, SchemeId::Winternitz, 0).is_err());
    // Spacing does not matter; the words do.
    let spaced = PHRASE.replace(' ', "   ");
    assert_eq!(first, phrase_seed(&spaced, SchemeId::Xmss, 0).unwrap());

    // Pinned: changing the derivation changes every key made from a phrase.
    assert_eq!(hex::encode(first), PINNED_SEED_0);
    let (public, _) = phrase_key(PHRASE, SchemeId::Xmss, 0).unwrap();
    assert_eq!(
        &hex::encode(Sha256::digest(&public))[..16],
        PINNED_KEY_0_HASH
    );
}

const PINNED_SEED_0: &str = "ed626d6c3921e6f9d30fa3b3ae7e826bf21fca1c2e449cf66c76776bfe084a5e";
const PINNED_KEY_0_HASH: &str = "61ecd8d18a520f32";

#[test]
fn the_key_an_account_holds_is_found_again_by_its_number() {
    let (second, _) = phrase_key(PHRASE, SchemeId::Xmss, 1).unwrap();
    assert_eq!(find_phrase_key(PHRASE, SchemeId::Xmss, &second, 2), Ok(1));
    assert_eq!(
        find_phrase_key(PHRASE, SchemeId::Xmss, &second, 0),
        Err(VectorError::NotFromPhrase { searched: 0 })
    );
}

#[test]
fn a_bad_phrase_and_a_key_that_never_moves_are_refused() {
    assert_eq!(
        phrase_seed("abandon abandon", SchemeId::Xmss, 0),
        Err(VectorError::BadPhrase)
    );
    let bad_checksum = PHRASE.replace("about", "abandon");
    assert_eq!(
        normalize_key_phrase(&bad_checksum),
        Err(VectorError::BadPhrase)
    );
    assert!(phrase_key(PHRASE, SchemeId::MlDsa44, 0).is_err());
    assert_eq!(new_key_phrase().split_whitespace().count(), 24);
}

#[test]
fn the_derivation_is_winterwallets_step_for_step() {
    // Winterwallet at 672fc67, core/tests/mnemonic.rs: under its magic
    // string, the node at wallet 0 / parent 0 / child 0, then its hardened
    // children 0 (the first scalar) and 17 (the last checksum scalar, N=16).
    let phrase = "earn foster affair make exclude object spring oppose one hollow garage kind";
    let (key, chain) = node(b"Winternitz seed", phrase, &[0, 0, 0]).unwrap();
    assert_eq!(
        hardened_child(&key, &chain, 0).0,
        [
            164, 137, 216, 51, 111, 169, 142, 239, 113, 82, 90, 41, 78, 207, 49, 227, 244, 190,
            165, 199, 162, 239, 152, 191, 15, 151, 156, 180, 239, 59, 94, 121,
        ]
    );
    assert_eq!(
        hardened_child(&key, &chain, 17).0,
        [
            66, 231, 115, 99, 93, 127, 202, 150, 2, 62, 233, 121, 122, 138, 241, 229, 8, 138, 133,
            41, 125, 139, 108, 9, 227, 127, 80, 87, 189, 114, 145, 60,
        ]
    );
}
