//! `resources/program-builds.toml`: which build of each program an embedded
//! IDL was reviewed against.

use serde::Deserialize;
use solana_pubkey::Pubkey;
use std::path::Path;
use std::str::FromStr;
use wallet_core::IdlNetwork;

/// The `programHash` of a program the validator runs itself.
pub const NATIVE: &str = "native";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    builds: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    program_id: String,
    network: String,
    program_hash: String,
    #[serde(default)]
    authority: Option<String>,
    idl_sha256: String,
    #[serde(default)]
    source: Option<SourceEntry>,
    #[serde(default)]
    verified: Option<String>,
    reviewed: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceEntry {
    repo: String,
    commit: String,
}

/// One reviewed tie between a program build and an embedded IDL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    pub program_id: Pubkey,
    pub network: IdlNetwork,
    /// `None` for a native program.
    pub program_hash: Option<[u8; 32]>,
    /// Who could change the program when it was reviewed. `None`: nobody.
    pub authority: Option<Pubkey>,
    /// SHA-256 of the embedded IDL file reviewed against this build.
    pub idl_sha256: [u8; 32],
    pub source: Option<(String, String)>,
    /// The uploader of the verify PDA whose build OtterSec reproduced.
    pub verified: Option<Pubkey>,
    pub reviewed: String,
}

pub fn read(path: &Path) -> Result<Vec<Build>, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    parse(&text).map_err(|err| format!("{}: {err}", path.display()))
}

pub fn parse(text: &str) -> Result<Vec<Build>, String> {
    let file: File = toml::from_str(text).map_err(|err| err.to_string())?;
    let mut builds: Vec<Build> = Vec::new();
    for entry in file.builds {
        let at = format!("{} on {}", entry.program_id, entry.network);
        let build = Build {
            program_id: pubkey(&entry.program_id).ok_or_else(|| format!("{at}: bad programId"))?,
            network: IdlNetwork::from_key(&entry.network)
                .ok_or_else(|| format!("{at}: unknown network"))?,
            program_hash: match entry.program_hash.as_str() {
                NATIVE => None,
                hash => Some(hex32(hash).ok_or_else(|| format!("{at}: bad programHash"))?),
            },
            authority: match &entry.authority {
                Some(key) => Some(pubkey(key).ok_or_else(|| format!("{at}: bad authority"))?),
                None => None,
            },
            idl_sha256: hex32(&entry.idl_sha256).ok_or_else(|| format!("{at}: bad idlSha256"))?,
            source: entry.source.map(|source| (source.repo, source.commit)),
            verified: match &entry.verified {
                Some(key) => Some(pubkey(key).ok_or_else(|| format!("{at}: bad verified"))?),
                None => None,
            },
            reviewed: entry.reviewed,
        };
        if builds
            .iter()
            .any(|seen| seen.program_id == build.program_id && seen.network == build.network)
        {
            return Err(format!("{at} is listed twice"));
        }
        builds.push(build);
    }
    Ok(builds)
}

fn pubkey(text: &str) -> Option<Pubkey> {
    Pubkey::from_str(text).ok()
}

fn hex32(text: &str) -> Option<[u8; 32]> {
    hex::decode(text).ok()?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = r#"
[[builds]]
programId = "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf"
network = "mainnet"
programHash = "d48660833989ecea3145ff726164fe640bd90696f03ce00dfd0cda258cbf2fac"
idlSha256 = "0000000000000000000000000000000000000000000000000000000000000001"
source = { repo = "https://github.com/Squads-Protocol/v4", commit = "2a47b4cc76068fb8accec3d5a599e6020c50f796" }
verified = "sqdcVVoTcKZjXU8yPUwKFbGx1Hig1rhbWJQtMRXp2E1"
reviewed = "2026-10-27"
"#;

    #[test]
    fn an_entry_reads_back() {
        let builds = parse(ENTRY).unwrap();
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].network, IdlNetwork::Mainnet);
        assert_eq!(builds[0].authority, None);
        assert!(builds[0].verified.is_some());
    }

    #[test]
    fn a_program_listed_twice_on_one_network_is_refused() {
        let twice = format!("{ENTRY}{ENTRY}");
        assert!(parse(&twice).unwrap_err().contains("twice"));
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let typo = ENTRY.replace("reviewed =", "reviewd =");
        assert!(parse(&typo).is_err());
    }

    #[test]
    fn the_checked_in_file_reads() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/program-builds.toml");
        read(&path).unwrap();
    }
}
