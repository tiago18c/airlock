//! Hash-based keys made from a recovery phrase, so that a lost device's
//! keys can be made again, and the one an account holds found again.
//!
//! The design is Blueshift's, from Winterwallet
//! (<https://github.com/blueshift-gg/winterwallet>, `core/src/keypair.rs`
//! at `672fc67`): a standard BIP-39 seed (PBKDF2, no passphrase), a master
//! node of `HMAC-SHA512(magic, seed)`, and hardened-only SLIP-0010 style
//! children along `wallet' / parent' / child'`, with `child` the number of
//! the key and `parent` 0. The child function is theirs byte for byte.
//!
//! What differs is what a node becomes. Winterwallet takes N + 2 more
//! hardened children of the node as the scalars of its own Winternitz key.
//! A Vector account verifies `solana-winternitz` keys, whose secrets are
//! sampled from a stream, so here the node's 32-byte key is the seed of that
//! stream ([`xmss_from_seed`], as for a seed made at random). The magic
//! string is ours as well, one per scheme, so one phrase used in both apps
//! shares no node:
//!
//! ```text
//! master = HMAC-SHA512("Vector XMSS seed", bip39_seed)
//!   └── wallet'  0
//!        └── parent'  0
//!             └── child'  0 for the key the account is made with, then one
//!                         more for each key it moves to
//! ```
//!
//! The known-answer test in `phrase_tests` pins all of it: changing any step
//! changes every key made from a phrase.

use super::keys::xmss_from_seed;
use super::scheme::SchemeId;
use super::VectorError;
use bip39::{Language, Mnemonic};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use zeroize::{Zeroize, Zeroizing};

/// How many keys past the one a record holds are tried when looking for
/// the key an account holds.
pub const PHRASE_SEARCH_AHEAD: u32 = 64;

/// The wallet a phrase's keys are made under. One phrase, one wallet.
const WALLET: u32 = 0;
/// Always 0, as in Winterwallet's CLI.
const PARENT: u32 = 0;

/// The magic string of `scheme`'s master node, for a scheme whose key
/// moves on.
fn magic(scheme: SchemeId) -> Result<&'static [u8], VectorError> {
    match scheme {
        SchemeId::Xmss => Ok(b"Vector XMSS seed"),
        SchemeId::Winternitz
        | SchemeId::Ed25519
        | SchemeId::Secp256k1
        | SchemeId::Eip191
        | SchemeId::MlDsa44
        | SchemeId::Falcon512 => Err(VectorError::CannotSign {
            scheme: scheme.label(),
        }),
    }
}

/// A new 24-word recovery phrase from the OS's randomness.
pub fn new_key_phrase() -> String {
    crate::keys::derive::new_phrase(24)
}

/// `phrase` with its words separated by single spaces, when it is a valid
/// English mnemonic.
pub fn normalize_key_phrase(phrase: &str) -> Result<String, VectorError> {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    if !matches!(words.len(), 12 | 15 | 18 | 21 | 24) {
        return Err(VectorError::BadPhrase);
    }
    let joined = words.join(" ");
    Mnemonic::parse_in_normalized(Language::English, &joined)
        .map_err(|_| VectorError::BadPhrase)?;
    Ok(joined)
}

/// The 32-byte seed of key number `index` of `scheme`, made from `phrase`.
pub fn phrase_seed(
    phrase: &str,
    scheme: SchemeId,
    index: u32,
) -> Result<Zeroizing<[u8; 32]>, VectorError> {
    let (key, mut chain) = node(magic(scheme)?, phrase, &[WALLET, PARENT, index])?;
    chain.zeroize();
    Ok(Zeroizing::new(key))
}

/// The node at the hardened `path` under the master node of `magic`, as
/// key and chain code. Winterwallet's derivation, step for step.
fn node(magic: &[u8], phrase: &str, path: &[u32]) -> Result<([u8; 32], [u8; 32]), VectorError> {
    let phrase = Zeroizing::new(normalize_key_phrase(phrase)?);
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, &phrase)
        .map_err(|_| VectorError::BadPhrase)?;
    let mut seed = mnemonic.to_seed("");
    let (mut key, mut chain) = split(hmac(magic, &[&seed]));
    seed.zeroize();
    for step in path {
        let (next_key, next_chain) = hardened_child(&key, &chain, *step);
        key.zeroize();
        chain.zeroize();
        (key, chain) = (next_key, next_chain);
    }
    Ok((key, chain))
}

/// `HMAC-SHA512(chain, 0x00 || key || ser32(index | 2^31))`, split into the
/// child's key and chain code.
fn hardened_child(key: &[u8; 32], chain: &[u8; 32], index: u32) -> ([u8; 32], [u8; 32]) {
    split(hmac(
        chain,
        &[&[0u8], key, &(index | 0x8000_0000).to_be_bytes()],
    ))
}

/// Key number `index` of `scheme` made from `phrase`: its public key, then
/// the seed a record keeps for it, wiped when dropped.
pub fn phrase_key(
    phrase: &str,
    scheme: SchemeId,
    index: u32,
) -> Result<(Vec<u8>, Zeroizing<[u8; 32]>), VectorError> {
    let seed = phrase_seed(phrase, scheme, index)?;
    let public = xmss_from_seed(&seed)?.verifying_key().to_bytes().to_vec();
    Ok((public, seed))
}

/// The number of the key `phrase` makes that is `key`, trying `0` to
/// `upto`, as Winterwallet's `recover` does. Every key is rebuilt to
/// compare, so this takes a moment.
pub fn find_phrase_key(
    phrase: &str,
    scheme: SchemeId,
    key: &[u8],
    upto: u32,
) -> Result<u32, VectorError> {
    for index in 0..=upto {
        if phrase_key(phrase, scheme, index)?.0 == key {
            return Ok(index);
        }
    }
    Err(VectorError::NotFromPhrase { searched: upto })
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 64] {
    let mut mac = <Hmac<Sha512>>::new_from_slice(key).expect("HMAC takes any key length");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

fn split(mut both: [u8; 64]) -> ([u8; 32], [u8; 32]) {
    let mut key = [0u8; 32];
    let mut chain = [0u8; 32];
    key.copy_from_slice(&both[..32]);
    chain.copy_from_slice(&both[32..]);
    both.zeroize();
    (key, chain)
}

#[cfg(test)]
#[path = "phrase_tests.rs"]
mod tests;
