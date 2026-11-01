//! Whether a password is worth sealing a vault with.
//!
//! Anyone who copies the file can try passwords for as long as they like. What
//! the key derivation sets is what each try costs, and that does not help a
//! password that is among the first tried. This is a floor: it refuses what a
//! search tries first, and it cannot say that a password it lets through is good.

use std::collections::BTreeSet;

const MIN_CHARACTERS: usize = 12;
/// Different characters a password needs, once what everyone uses is taken out.
const MIN_DIFFERENT: usize = 6;

/// What a search tries first, in lower case.
const COMMON: &[&str] = &[
    "password", "passwd", "passw0rd", "p4ssw0rd", "qwerty", "qwertz", "azerty", "asdf", "zxcv",
    "abcd", "0123", "1234", "2345", "3456", "4567", "5678", "6789", "7890", "9876", "8765", "7654",
    "6543", "5432", "4321", "letmein", "welcome", "admin", "login", "iloveyou", "dragon", "monkey",
    "master", "secret", "changeme", "airlock", "solana", "wallet", "bitcoin", "crypto",
];

/// Why `password` should not seal a vault, if it should not.
pub fn password_problem(password: &str) -> Option<&'static str> {
    if password.chars().count() < MIN_CHARACTERS {
        return Some(
            "Use at least 12 characters. A few random words are easy to type and hard to guess.",
        );
    }
    let mut rest = password.to_lowercase();
    for common in COMMON {
        rest = rest.replace(common, "");
    }
    if rest.chars().collect::<BTreeSet<_>>().len() < MIN_DIFFERENT {
        return Some(
            "That is too easy to guess: it repeats itself, or is made of common words or patterns. A few random words are easy to type and hard to guess.",
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_search_tries_first_is_refused() {
        for password in [
            "",
            "short",
            "elevenchars",
            "aaaaaaaaaaaa",
            "abababababab",
            "qwertyqwerty",
            "123456789012",
            "password1234",
            "Password1234!",
            "PASSWORDPASSWORD",
            "solanawallet12",
            "abcdabcdabcd",
        ] {
            assert!(password_problem(password).is_some(), "{password}");
        }
    }

    #[test]
    fn a_long_password_that_is_not_made_of_those_is_let_through() {
        for password in [
            "correct horse battery",
            "Tjf8#kLq2!pZ",
            "pão de queijo quente",
            "my password is a long sentence",
        ] {
            assert_eq!(password_problem(password), None, "{password}");
        }
    }

    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        // Eleven different characters, in many more bytes.
        assert!(password_problem("áéíóúàèìòùâ").is_some());
        // Twelve of them.
        assert_eq!(password_problem("áéíóúàèìòùâê"), None);
    }
}
