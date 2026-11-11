//! The size of a Token-2022 mint, from the extensions a template initializes
//! on it.
//!
//! Token-2022 lays a mint with extensions out as the 82-byte base padded to
//! the 165 bytes of an account, one byte for the account type, then each
//! extension as a 2-byte type, a 2-byte length and its value. A mint with no
//! extension is the bare 82 bytes. A length that would equal a multisig's
//! (355) gets two bytes more, so the two cannot be told apart by length.
//! `InitializeMint` refuses an account whose length is not exactly this.
//!
//! Each fixed length below was checked by creating a mint with that
//! extension against the Token-2022 program the flow tests run.

/// The instructions that initialize a mint extension of a fixed length, and
/// that length.
pub const MINT_EXTENSIONS: [(&str, usize); 11] = [
    ("initializeTransferFeeConfig", 108),
    ("initializeMintCloseAuthority", 32),
    ("initializeNonTransferableMint", 0),
    ("initializeInterestBearingMint", 52),
    ("initializePermanentDelegate", 32),
    ("initializeTransferHook", 64),
    ("initializeMetadataPointer", 64),
    ("initializeGroupPointer", 64),
    ("initializeGroupMemberPointer", 64),
    ("initializePausableConfig", 33),
    ("initializeScaledUiAmountMint", 56),
];

/// Writes token metadata on the mint, growing it as it does.
pub const TOKEN_METADATA: &str = "initializeTokenMetadata";

/// Token-2022 instructions named `initialize…` that set up no mint
/// extension.
const NOT_MINT_EXTENSIONS: [&str; 8] = [
    "initializeMint",
    "initializeMint2",
    "initializeAccount",
    "initializeAccount2",
    "initializeAccount3",
    "initializeMultisig",
    "initializeMultisig2",
    "initializeImmutableOwner",
];

const MINT_LEN: usize = 82;
const ACCOUNT_LEN: usize = 165;
const MULTISIG_LEN: usize = 355;
const TLV_HEADER: usize = 4;

/// What an instruction does to a mint's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sizing {
    /// Sets up no extension.
    None,
    /// An extension of this length.
    Fixed(usize),
    /// Token metadata, whose length is its text's.
    Metadata,
    /// An extension this table does not know.
    Unknown,
}

pub(crate) fn sizing(instruction: &str) -> Sizing {
    if let Some((_, len)) = MINT_EXTENSIONS
        .iter()
        .find(|(name, _)| *name == instruction)
    {
        return Sizing::Fixed(*len);
    }
    if instruction == TOKEN_METADATA {
        return Sizing::Metadata;
    }
    if instruction.starts_with("initialize") && !NOT_MINT_EXTENSIONS.contains(&instruction) {
        return Sizing::Unknown;
    }
    Sizing::None
}

/// The length of a mint holding extensions of these value lengths.
pub(crate) fn mint_len(extensions: &[usize]) -> usize {
    if extensions.is_empty() {
        return MINT_LEN;
    }
    let tlv: usize = extensions.iter().map(|len| TLV_HEADER + len).sum();
    let len = ACCOUNT_LEN + 1 + tlv;
    if len == MULTISIG_LEN {
        len + 2
    } else {
        len
    }
}

/// The value length of token metadata with this text and no additional
/// fields: update authority and mint, three strings each after a 4-byte
/// length, and the 4-byte length of the empty list of additional fields.
pub(crate) fn metadata_len(name: &str, symbol: &str, uri: &str) -> usize {
    32 + 32 + 4 + name.len() + 4 + symbol.len() + 4 + uri.len() + 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mint_with_no_extension_is_the_bare_base() {
        assert_eq!(mint_len(&[]), 82);
        assert_eq!(mint_len(&[0]), 170);
        assert_eq!(mint_len(&[108, 32]), 166 + 112 + 36);
    }

    #[test]
    fn a_length_equal_to_a_multisig_grows_by_an_extension_type() {
        // 166 + 189 = 355.
        assert_eq!(mint_len(&[185]), 357);
        assert_eq!(mint_len(&[184]), 354);
    }

    #[test]
    fn instructions_are_sorted_into_what_they_do_to_the_size() {
        assert_eq!(sizing("initializeMint2"), Sizing::None);
        assert_eq!(sizing("mintToChecked"), Sizing::None);
        assert_eq!(sizing("initializePermanentDelegate"), Sizing::Fixed(32));
        assert_eq!(sizing("initializeTokenMetadata"), Sizing::Metadata);
        assert_eq!(sizing("initializeTokenGroup"), Sizing::Unknown);
        assert_eq!(sizing("initializeDefaultAccountState"), Sizing::Unknown);
    }
}
