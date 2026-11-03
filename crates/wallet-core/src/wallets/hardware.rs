//! Hardware wallets: a key on a Ledger, Trezor or Keystone that the user
//! holds. The vault keeps no secret of it, only what names the key and the
//! device, so the record shows and routes while the vault is locked.
//!
//! Airlock builds v1 transactions, so a device has to read v1. The first
//! versions that do are the minimums here: Ledger's Solana app 1.18, Trezor
//! 2.13.1 and Keystone 3.0.6. Each of them also signs off-chain messages in
//! the v1 format. See `docs/plan-hardware-wallets.md`.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HardwareVendor {
    Ledger,
    Trezor,
    Keystone,
}

impl HardwareVendor {
    pub const ALL: [Self; 3] = [Self::Ledger, Self::Trezor, Self::Keystone];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ledger => "Ledger",
            Self::Trezor => "Trezor",
            Self::Keystone => "Keystone",
        }
    }

    /// What the version of [`DeviceSeen::firmware`] is the version of.
    pub fn versioned(self) -> &'static str {
        match self {
            Self::Ledger => "Solana app",
            Self::Trezor | Self::Keystone => "firmware",
        }
    }

    /// The device answers over a cable, within a blockhash's life. A
    /// Keystone answers through QR codes, as another device does, so it
    /// votes in a multisig the way an offline key does.
    pub fn answers_in_time(self) -> bool {
        match self {
            Self::Ledger | Self::Trezor => true,
            Self::Keystone => false,
        }
    }

    /// The first version that reads v1 transactions.
    pub fn minimum(self) -> Version {
        match self {
            Self::Ledger => Version::new(1, 18, 0),
            Self::Trezor => Version::new(2, 13, 1),
            Self::Keystone => Version::new(3, 0, 6),
        }
    }

    /// Whether `seen` is a version this app signs with. An unknown version
    /// is not refused here: the device is asked when it connects.
    pub fn supports(self, seen: Option<Version>) -> bool {
        seen.is_none_or(|version| version >= self.minimum())
    }

    /// Why a device at `version` is refused, or `None` when it is not.
    pub fn refusal(self, version: Version) -> Option<String> {
        (version < self.minimum()).then(|| {
            format!(
                "This {} has {} {version}. Airlock needs {} or later, the first that reads v1 transactions.",
                self.label(),
                self.versioned(),
                self.minimum(),
            )
        })
    }
}

/// A `major.minor.patch` version, as devices report them. A pre-release
/// suffix (`1.18.0-dev2`) is dropped: a developer build counts as the
/// version it leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl Version {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("\"{0}\" is not a version.")]
pub struct BadVersion(String);

impl FromStr for Version {
    type Err = BadVersion;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || BadVersion(text.to_string());
        let core = text
            .trim()
            .trim_start_matches(['v', 'V'])
            .split(['-', '+', ' '])
            .next()
            .unwrap_or_default();
        let mut parts = core.split('.');
        let mut next = |required: bool| match parts.next() {
            Some(part) => part.parse::<u16>().map_err(|_| bad()),
            None if required => Err(bad()),
            None => Ok(0),
        };
        let version = Version::new(next(true)?, next(true)?, next(false)?);
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(version)
    }
}

impl Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// What Airlock last saw of the device: updated on every connect.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSeen {
    /// The model, such as "Nano S Plus" or "Safe 5".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The Solana app's version on a Ledger, the firmware's on the others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<Version>,
}

/// A way to number accounts on a device, with `*` for the account index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathScheme {
    pub template: &'static str,
    /// Who else numbers accounts this way, for the list.
    pub used_by: &'static str,
}

impl PathScheme {
    /// The concrete path of account `index`.
    pub fn path(&self, index: u32) -> String {
        self.template.replace('*', &index.to_string())
    }
}

/// The schemes Add wallet offers, the most common first.
pub const PATH_SCHEMES: &[PathScheme] = &[
    PathScheme {
        template: "m/44'/501'/*'/0'",
        used_by: "Phantom, Solflare, Trezor Suite and Airlock's own seed phrases",
    },
    PathScheme {
        template: "m/44'/501'/*'",
        used_by: "Ledger Live",
    },
    PathScheme {
        template: "m/44'/501'",
        used_by: "one account only",
    },
];

/// The indices of a path such as `m/44'/501'/0'/0'`, hardened ones with the
/// top bit set, as Ledger and Trezor take them. Only paths under
/// `44'/501'`, of two to five indices, are accepted: the Solana apps derive
/// nothing else.
pub fn parse_path(path: &str) -> Result<Vec<u32>, BadPath> {
    let bad = || BadPath(path.to_string());
    let rest = path.trim().strip_prefix("m/").ok_or_else(bad)?;
    let indices = rest
        .split('/')
        .map(|part| {
            let (number, hardened) = match part.strip_suffix(['\'', 'h', 'H']) {
                Some(number) => (number, true),
                None => (part, false),
            };
            let index = number.parse::<u32>().map_err(|_| bad())?;
            if index >= HARDENED {
                return Err(bad());
            }
            Ok(if hardened { index | HARDENED } else { index })
        })
        .collect::<Result<Vec<u32>, BadPath>>()?;
    let solana = [44 | HARDENED, 501 | HARDENED];
    if !(2..=5).contains(&indices.len()) || indices[..2] != solana {
        return Err(bad());
    }
    Ok(indices)
}

/// The bit that marks a hardened index.
pub const HARDENED: u32 = 0x8000_0000;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("\"{0}\" is not a Solana path. It starts with m/44'/501' and has at most five parts.")]
pub struct BadPath(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_developer_build_counts_as_the_version_it_leads_to() {
        assert_eq!("1.18.0-dev2".parse(), Ok(Version::new(1, 18, 0)));
        assert_eq!("v2.13.1".parse(), Ok(Version::new(2, 13, 1)));
        assert_eq!("3.2".parse(), Ok(Version::new(3, 2, 0)));
        assert!("1.18.0.4".parse::<Version>().is_err());
        assert!("one".parse::<Version>().is_err());
    }

    #[test]
    fn a_device_below_the_first_version_that_reads_v1_is_refused() {
        let ledger = HardwareVendor::Ledger;
        assert!(ledger.refusal(Version::new(1, 16, 1)).is_some());
        assert!(ledger.refusal(Version::new(1, 18, 0)).is_none());
        assert!(HardwareVendor::Trezor
            .refusal(Version::new(2, 13, 0))
            .is_some());
        assert!(HardwareVendor::Keystone
            .refusal(Version::new(3, 2, 0))
            .is_none());
        assert!(ledger.supports(None));
        assert!(!ledger.supports(Some(Version::new(1, 17, 0))));
    }

    #[test]
    fn a_version_is_stored_as_text() {
        let seen = DeviceSeen {
            model: Some("Safe 5".into()),
            firmware: Some(Version::new(2, 13, 1)),
        };
        let json = serde_json::to_string(&seen).unwrap();
        assert_eq!(json, r#"{"model":"Safe 5","firmware":"2.13.1"}"#);
        assert_eq!(serde_json::from_str::<DeviceSeen>(&json).unwrap(), seen);
    }

    #[test]
    fn only_solana_paths_are_taken() {
        assert_eq!(
            parse_path("m/44'/501'/3'/0'"),
            Ok(vec![44 | HARDENED, 501 | HARDENED, 3 | HARDENED, HARDENED])
        );
        assert_eq!(parse_path("m/44'/501'").map(|path| path.len()), Ok(2));
        assert!(parse_path("m/44'/60'/0'").is_err());
        assert!(parse_path("44'/501'/0'").is_err());
        assert!(parse_path("m/44'/501'/0'/0'/0'/0'").is_err());
        assert_eq!(PATH_SCHEMES[0].path(2), "m/44'/501'/2'/0'");
    }
}
