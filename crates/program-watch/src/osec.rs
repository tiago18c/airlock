//! OtterSec's verified-build API. Anyone may upload a verify PDA naming a
//! repository and commit for a program. OtterSec builds that commit and
//! records whether the executable matches the one on chain. It covers
//! mainnet only.

use serde::Deserialize;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use std::time::Duration;

const API: &str = "https://verify.osec.io";

#[derive(Debug, Deserialize)]
struct RawEntry {
    signer: Option<String>,
    is_verified: bool,
    #[serde(default)]
    executable_hash: String,
    #[serde(default)]
    repo_url: String,
    #[serde(default)]
    commit: String,
}

/// One build OtterSec ran for a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    /// Who uploaded the verify PDA. `None` for builds submitted before PDAs.
    pub signer: Option<Pubkey>,
    pub verified: bool,
    /// What the build of `commit` hashed to.
    pub executable_hash: Option<[u8; 32]>,
    pub repo: String,
    pub commit: String,
}

pub struct Client {
    http: reqwest::Client,
}

impl Client {
    pub fn new() -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("airlock-program-watch")
            .build()
            .map_err(|err| err.to_string())?;
        Ok(Self { http })
    }

    /// Every build OtterSec ran for `program_id`, whatever their result.
    pub async fn builds(&self, program_id: &Pubkey) -> Result<Vec<Verification>, String> {
        let url = format!("{API}/status-all/{program_id}");
        let entries: Vec<RawEntry> = self
            .http
            .get(&url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|err| format!("{url}: {err}"))?
            .json()
            .await
            .map_err(|err| format!("{url}: {err}"))?;
        Ok(entries
            .into_iter()
            .map(|entry| Verification {
                signer: entry
                    .signer
                    .as_deref()
                    .and_then(|key| Pubkey::from_str(key).ok()),
                verified: entry.is_verified,
                executable_hash: hex::decode(&entry.executable_hash)
                    .ok()
                    .and_then(|bytes| bytes.try_into().ok()),
                // The API names the tree at the commit. The repository is
                // what comes before it.
                repo: entry
                    .repo_url
                    .split("/tree/")
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                commit: entry.commit,
            })
            .collect())
    }
}

/// The verified build of `hash`, preferring one uploaded by `authority`, the
/// one who can deploy the program and so speaks for it.
pub fn verified_build<'a>(
    builds: &'a [Verification],
    hash: &[u8; 32],
    authority: Option<&Pubkey>,
) -> Option<&'a Verification> {
    let mut matching = builds
        .iter()
        .filter(|build| build.verified && build.executable_hash.as_ref() == Some(hash));
    let first = matching.clone().next();
    matching
        .find(|build| authority.is_some() && build.signer.as_ref() == authority)
        .or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(signer: u8, verified: bool, hash: u8) -> Verification {
        Verification {
            signer: Some(Pubkey::new_from_array([signer; 32])),
            verified,
            executable_hash: Some([hash; 32]),
            repo: "https://github.com/x/y".into(),
            commit: "c".into(),
        }
    }

    #[test]
    fn only_a_verified_build_of_the_running_code_counts() {
        let builds = [build(1, false, 7), build(2, true, 8)];
        assert_eq!(verified_build(&builds, &[7; 32], None), None);
        assert_eq!(verified_build(&builds, &[8; 32], None), Some(&builds[1]));
    }

    #[test]
    fn the_authority_upload_wins_over_another() {
        let builds = [build(1, true, 7), build(2, true, 7)];
        let authority = Pubkey::new_from_array([2; 32]);
        assert_eq!(
            verified_build(&builds, &[7; 32], Some(&authority)),
            Some(&builds[1])
        );
        assert_eq!(verified_build(&builds, &[7; 32], None), Some(&builds[0]));
    }
}
