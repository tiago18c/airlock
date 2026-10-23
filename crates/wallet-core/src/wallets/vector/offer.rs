//! Which schemes this build offers, and which it can keep a key of.
//!
//! Every build reads every scheme. This answers only what the app builds: a
//! Vector wallet already in the vault is listed either way, and a
//! transaction for one is explained either way.

use super::SchemeId;

/// Whether a wallet of `scheme` may be added at all, with its key here or
/// elsewhere.
///
/// Winternitz is not: a key signs once, and a transaction is all or
/// nothing. When what it authorized fails on chain (a swap past its
/// slippage, an account that moved), the account still holds the key, and
/// anything else needs a second signature from it, which weakens it. So an
/// account cannot leave a failed transaction behind safely.
pub fn scheme_offered(scheme: SchemeId) -> bool {
    match scheme {
        SchemeId::Winternitz => false,
        SchemeId::Ed25519
        | SchemeId::Secp256k1
        | SchemeId::Eip191
        | SchemeId::MlDsa44
        | SchemeId::Falcon512
        | SchemeId::Xmss => true,
    }
}

/// Whether this build may keep a key of `scheme` and sign with it.
///
/// A key that signs once per leaf has to remember which leaves it used. A
/// browser can clear its storage or sync an older copy back, so the web
/// build keeps no such key. Falcon-512 is desktop and airgap only (D3): the
/// crate is unaudited and uses floating point. A scheme that is not
/// [`scheme_offered`] signs nowhere.
pub fn signs_here(scheme: SchemeId) -> bool {
    match scheme {
        SchemeId::Winternitz => false,
        SchemeId::Falcon512 | SchemeId::Xmss => !cfg!(target_arch = "wasm32"),
        SchemeId::Ed25519 | SchemeId::Secp256k1 | SchemeId::Eip191 | SchemeId::MlDsa44 => true,
    }
}
