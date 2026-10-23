//! Anchor discriminator helpers.
//!
//! v01 IDLs carry explicit discriminator byte arrays; v00 IDLs compute them as
//! `sha256(preimage)[..8]` where the preimage is `global:<snake_case name>` for
//! instructions, `account:<PascalCase name>` for accounts and
//! `event:<PascalCase name>` for events.

use crate::helpers::{pascal_case, snake_case};
use sha2::{Digest, Sha256};

fn sha256_prefix(preimage: String) -> Vec<u8> {
    Sha256::digest(preimage.into_bytes())[..8].to_vec()
}

pub fn anchor_instruction_discriminator(name: &str) -> Vec<u8> {
    sha256_prefix(format!("global:{}", snake_case(name)))
}

pub fn anchor_account_discriminator(name: &str) -> Vec<u8> {
    sha256_prefix(format!("account:{}", pascal_case(name)))
}

pub fn anchor_event_discriminator(name: &str) -> Vec<u8> {
    sha256_prefix(format!("event:{}", pascal_case(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors from the TS package's discriminator tests.
    #[test]
    fn matches_ts_vectors() {
        assert_eq!(
            hex::encode(anchor_account_discriminator("StakeEntry")),
            "bb7f09239b445628"
        );
        assert_eq!(
            hex::encode(anchor_event_discriminator("MyEvent")),
            "60b8c5f38b025a94"
        );
        assert_eq!(
            hex::encode(anchor_instruction_discriminator("addConfigLines")),
            "df32e0e39708736a"
        );
    }

    #[test]
    fn matches_squads_accounts() {
        assert_eq!(
            hex::encode(anchor_account_discriminator("VaultTransaction")),
            "a8faa264510ea2cf"
        );
        assert_eq!(
            hex::encode(anchor_account_discriminator("Proposal")),
            "1a5ebdbb74883521"
        );
    }
}
