use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

/// 16-byte wallet id, stored as hex in JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WalletId(#[serde(with = "hex_bytes")] pub [u8; 16]);

impl WalletId {
    pub fn new() -> Self {
        let mut bytes = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
        Self(bytes)
    }
}

impl Default for WalletId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WalletId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

pub(crate) mod pubkey_string {
    use serde::{Deserialize, Deserializer, Serializer};
    use solana_pubkey::Pubkey;
    use std::str::FromStr;

    pub fn serialize<S: Serializer>(pubkey: &Pubkey, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&pubkey.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Pubkey, D::Error> {
        let text = String::deserialize(deserializer)?;
        Pubkey::from_str(&text).map_err(serde::de::Error::custom)
    }
}

pub(crate) mod option_pubkey_string {
    use serde::{Deserialize, Deserializer, Serializer};
    use solana_pubkey::Pubkey;
    use std::str::FromStr;

    pub fn serialize<S: Serializer>(
        pubkey: &Option<Pubkey>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match pubkey {
            Some(pubkey) => serializer.serialize_some(&pubkey.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Pubkey>, D::Error> {
        let value = Option::<String>::deserialize(deserializer)?;
        value
            .map(|text| Pubkey::from_str(&text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 16], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 16], D::Error> {
        let s = String::deserialize(deserializer)?;
        let v = hex::decode(&s).map_err(serde::de::Error::custom)?;
        let arr: [u8; 16] = v
            .try_into()
            .map_err(|_| serde::de::Error::custom("wallet id must be 16 bytes"))?;
        Ok(arr)
    }
}

pub const SYSTEM_PROGRAM_ID: Pubkey =
    solana_pubkey::Pubkey::from_str_const("11111111111111111111111111111111");

pub const STAKE_PROGRAM_ID: Pubkey =
    solana_pubkey::Pubkey::from_str_const("Stake11111111111111111111111111111111111111");

pub const TOKEN_PROGRAM_ID: Pubkey =
    solana_pubkey::Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

pub const TOKEN_2022_PROGRAM_ID: Pubkey =
    solana_pubkey::Pubkey::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

pub const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    solana_pubkey::Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

pub const COMPUTE_BUDGET_PROGRAM_ID: Pubkey =
    solana_pubkey::Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");

pub const SQUADS_V4_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");
pub const SQUADS_SMART_ACCOUNT_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("SMRTzfY6DfH5ik3TKiyLFfXexV8uSG3d2UksSCYdunG");

pub fn short_pubkey(pubkey: &Pubkey) -> String {
    let s = pubkey.to_string();
    if s.len() <= 10 {
        s
    } else {
        format!("{}…{}", &s[..5], &s[s.len() - 5..])
    }
}
