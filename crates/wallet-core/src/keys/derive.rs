//! Seed-phrase derivation for Solana signer accounts.
//!
//! A `*` in the path is the account index. `m/44'/501'/*'/0'` at index 3 is
//! `m/44'/501'/3'/0'`. Paths without `*` name a single account.

use crate::error::DeriveError;
use bip39::{Language, Mnemonic};
use solana_derivation_path::DerivationPath;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_seed_derivable::SeedDerivable;

pub const SOLANA_PATH_TEMPLATE: &str = "m/44'/501'/*'/0'";
pub const DERIVE_BATCH: u32 = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedAccount {
    pub index: u32,
    pub path: String,
    pub pubkey: Pubkey,
}

pub fn path_has_wildcard(template: &str) -> bool {
    template.contains('*')
}

/// Replace the single `*` with `index`. A path without `*` is only valid at index 0.
pub fn concrete_path(template: &str, index: u32) -> Result<String, DeriveError> {
    let template = template.trim();
    if template.is_empty() {
        return Err(DeriveError::PathRequired);
    }
    if !template.contains('*') {
        if index != 0 {
            return Err(DeriveError::NoWildcard);
        }
        parse_path(template)?;
        return Ok(template.to_string());
    }
    let expanded = template.replacen('*', &index.to_string(), 1);
    if expanded.contains('*') {
        return Err(DeriveError::ManyWildcards);
    }
    parse_path(&expanded)?;
    Ok(expanded)
}

pub fn derive_batch(
    phrase: &str,
    passphrase: &str,
    template: &str,
    start: u32,
    count: u32,
) -> Result<Vec<DerivedAccount>, DeriveError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let phrase = zeroize::Zeroizing::new(normalize_phrase(phrase)?);
    let seed = mnemonic_seed(&phrase, passphrase)?;
    let wildcard = path_has_wildcard(template);
    let count = if wildcard { count } else { 1 };
    if !wildcard && start != 0 {
        return Err(DeriveError::NoWildcard);
    }
    let mut out = Vec::with_capacity(count as usize);
    for offset in 0..count {
        let index = start + offset;
        let path = concrete_path(template, index)?;
        let pubkey = pubkey_at(&seed, &path)?;
        out.push(DerivedAccount {
            index,
            path,
            pubkey,
        });
    }
    Ok(out)
}

/// A new English seed phrase of 12 or 24 words, from the system's random
/// source. Any other count gives 12.
pub fn new_phrase(words: usize) -> String {
    use rand::RngCore;
    use zeroize::Zeroize;
    let mut entropy = [0u8; 32];
    let used = if words == 24 { 32 } else { 16 };
    rand::rngs::OsRng.fill_bytes(&mut entropy[..used]);
    let phrase = Mnemonic::from_entropy_in(Language::English, &entropy[..used])
        .expect("16 and 32 bytes are valid entropy lengths")
        .to_string();
    entropy.zeroize();
    phrase
}

pub fn keypair_at(phrase: &str, passphrase: &str, path: &str) -> Result<Keypair, DeriveError> {
    let phrase = zeroize::Zeroizing::new(normalize_phrase(phrase)?);
    let seed = mnemonic_seed(&phrase, passphrase)?;
    let derivation = parse_path(path)?;
    Keypair::from_seed_and_derivation_path(seed.as_slice(), Some(derivation))
        .map_err(|err| DeriveError::Key(err.to_string()))
}

/// 32-byte ed25519 seed from a pasted key: JSON array, hex, or base58.
pub fn secret_seed_from_import(raw: &str) -> Result<[u8; 32], DeriveError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(DeriveError::KeyRequired);
    }
    let bytes = if raw.starts_with('[') {
        serde_json::from_str::<Vec<u8>>(raw).map_err(|err| DeriveError::KeyJson(err.to_string()))?
    } else if is_hex_seed(raw) {
        hex::decode(raw.strip_prefix("0x").unwrap_or(raw)).map_err(|_| DeriveError::KeyHex)?
    } else {
        bs58::decode(raw)
            .into_vec()
            .map_err(|_| DeriveError::KeyBase58)?
    };
    seed_from_bytes(&bytes)
}

fn normalize_phrase(phrase: &str) -> Result<String, DeriveError> {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    if !matches!(words.len(), 12 | 15 | 18 | 21 | 24) {
        return Err(DeriveError::PhraseLength);
    }
    let joined = words.join(" ");
    Mnemonic::parse_in_normalized(Language::English, &joined)
        .map_err(|_| DeriveError::PhraseInvalid)?;
    Ok(joined)
}

/// The BIP-39 seed, wiped when dropped.
fn mnemonic_seed(
    phrase: &str,
    passphrase: &str,
) -> Result<zeroize::Zeroizing<[u8; 64]>, DeriveError> {
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, phrase)
        .map_err(|_| DeriveError::PhraseInvalid)?;
    Ok(zeroize::Zeroizing::new(mnemonic.to_seed(passphrase)))
}

fn pubkey_at(seed: &[u8; 64], path: &str) -> Result<Pubkey, DeriveError> {
    let derivation = parse_path(path)?;
    let keypair = Keypair::from_seed_and_derivation_path(seed, Some(derivation))
        .map_err(|err| DeriveError::Key(err.to_string()))?;
    Ok(solana_signer::Signer::pubkey(&keypair))
}

fn parse_path(path: &str) -> Result<DerivationPath, DeriveError> {
    DerivationPath::from_absolute_path_str(path).map_err(|err| DeriveError::Path(err.to_string()))
}

fn is_hex_seed(raw: &str) -> bool {
    let hex = raw.strip_prefix("0x").unwrap_or(raw);
    matches!(hex.len(), 64 | 128) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn seed_from_bytes(bytes: &[u8]) -> Result<[u8; 32], DeriveError> {
    match bytes.len() {
        64 => {
            let keypair =
                Keypair::try_from(bytes).map_err(|err| DeriveError::Key(err.to_string()))?;
            Ok(*keypair.secret_bytes())
        }
        32 => {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(bytes);
            Ok(seed)
        }
        _ => Err(DeriveError::KeyLength),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABANDON_24: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

    #[test]
    fn solana_path_matches_the_published_abandon_vector() {
        let accounts = derive_batch(ABANDON_24, "", "m/44'/501'/*'/0'", 0, 2).unwrap();
        assert_eq!(accounts[0].path, "m/44'/501'/0'/0'");
        assert_eq!(
            accounts[0].pubkey.to_string(),
            "3Cy3YNTFywCmxoxt8n7UH6hg6dLo5uACowX3CFceaSnx"
        );
        assert_ne!(accounts[0].pubkey, accounts[1].pubkey);
        assert_eq!(accounts[1].path, "m/44'/501'/1'/0'");
    }

    #[test]
    fn a_new_phrase_is_valid_and_not_repeated() {
        for words in [12, 24] {
            let phrase = new_phrase(words);
            assert_eq!(phrase.split_whitespace().count(), words);
            assert_eq!(
                derive_batch(&phrase, "", SOLANA_PATH_TEMPLATE, 0, 1)
                    .unwrap()
                    .len(),
                1
            );
            assert_ne!(phrase, new_phrase(words));
        }
        assert_eq!(new_phrase(7).split_whitespace().count(), 12);
    }

    #[test]
    fn path_without_a_star_is_a_single_account() {
        let once = derive_batch(ABANDON_24, "", "m/44'/501'/0'/0'", 0, 10).unwrap();
        assert_eq!(once.len(), 1);
        assert!(derive_batch(ABANDON_24, "", "m/44'/501'/0'/0'", 1, 1).is_err());
    }

    #[test]
    fn signer_path_roundtrips_the_same_pubkey() {
        let account = &derive_batch(ABANDON_24, "secret", SOLANA_PATH_TEMPLATE, 4, 1).unwrap()[0];
        let keypair = keypair_at(ABANDON_24, "secret", &account.path).unwrap();
        assert_eq!(solana_signer::Signer::pubkey(&keypair), account.pubkey);
    }
}
