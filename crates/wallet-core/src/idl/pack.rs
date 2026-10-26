//! Reading the IDL pack: the third-party IDLs that ship beside the binary in
//! one compressed file. Its format is the `idl-pack` crate's. Its manifest
//! lists, for each file, what [`EmbeddedIdl`] needs.

use crate::idl::builtin::{EmbeddedIdl, IdlSource};
use crate::idl::catalog::{sha256_bytes, IdlChainSource, IdlOrigin};
use crate::idl::network::IdlNetwork;
use serde::Deserialize;
use solana_pubkey::Pubkey;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdlPackError {
    /// The pack is not the one this build was made with: a file changed on
    /// disk, or a server sent something else.
    #[error("the IDL pack is not the one this build ships (SHA-256 {found})")]
    Checksum { found: String },
    #[error(transparent)]
    Pack(#[from] idl_pack::PackError),
    #[error("the IDL pack's manifest is unreadable: {0}")]
    Manifest(String),
    #[error("the IDL pack's manifest lists {0}, which the pack does not hold")]
    Missing(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    idls: Vec<ManifestEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestEntry {
    file: String,
    program_id: String,
    name: String,
    networks: Vec<IdlNetwork>,
    source: ManifestSource,
    changes: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum ManifestSource {
    Chain {
        network: IdlNetwork,
        account: ChainAccount,
        sha256: String,
    },
    Repository {
        url: String,
    },
    Hand,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum ChainAccount {
    ProgramMetadata,
    Anchor,
}

/// The IDLs in `packed`, in the order the manifest lists them. A pack whose
/// SHA-256 is not `expected` is refused whole, before anything in it is read.
pub fn idls_from_pack(
    packed: &[u8],
    expected: &[u8; 32],
) -> Result<Vec<EmbeddedIdl>, IdlPackError> {
    let found = sha256_bytes(packed);
    if found != *expected {
        return Err(IdlPackError::Checksum {
            found: hex::encode(found),
        });
    }
    let mut files: BTreeMap<String, Vec<u8>> = idl_pack::unpack(packed)?.into_iter().collect();
    let manifest = files
        .remove(idl_pack::MANIFEST)
        .ok_or_else(|| IdlPackError::Missing(idl_pack::MANIFEST.into()))?;
    let manifest: Manifest =
        serde_json::from_slice(&manifest).map_err(|err| IdlPackError::Manifest(err.to_string()))?;
    manifest
        .idls
        .into_iter()
        .map(|entry| {
            let program_id = Pubkey::from_str(&entry.program_id)
                .map_err(|_| IdlPackError::Manifest(format!("{}: bad programId", entry.file)))?;
            let source = match entry.source {
                ManifestSource::Chain {
                    network,
                    account,
                    sha256,
                } => IdlSource::Chain(IdlChainSource {
                    network,
                    account: match account {
                        ChainAccount::ProgramMetadata => IdlOrigin::ProgramMetadata,
                        ChainAccount::Anchor => IdlOrigin::Anchor,
                    },
                    sha256: hex::decode(&sha256)
                        .ok()
                        .and_then(|bytes| bytes.try_into().ok())
                        .ok_or_else(|| {
                            IdlPackError::Manifest(format!("{}: bad source sha256", entry.file))
                        })?,
                }),
                ManifestSource::Repository { url } => IdlSource::Repository {
                    url: Cow::Owned(url),
                },
                ManifestSource::Hand => IdlSource::Hand,
            };
            let json = files
                .remove(&entry.file)
                .ok_or_else(|| IdlPackError::Missing(entry.file.clone()))?;
            Ok(EmbeddedIdl {
                program_id,
                name: entry.name,
                json: Arc::from(json),
                networks: entry.networks,
                source,
                changes: entry.changes,
            })
        })
        .collect()
}

/// The pack built from `resources/idls`, as the app ships it.
#[cfg(test)]
pub(crate) fn shipped_idls() -> Vec<EmbeddedIdl> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/idls");
    let packed = idl_pack::pack_dir(&dir).expect("resources/idls packs");
    idls_from_pack(&packed.bytes, &packed.sha256).expect("the pack reads back")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codama_dynamic_client::ProgramClient;

    #[test]
    fn a_pack_with_another_checksum_is_refused_unread() {
        let packed = idl_pack::pack(&[(idl_pack::MANIFEST, b"not json")]);
        let err = idls_from_pack(&packed, &[0; 32]).unwrap_err();
        assert!(matches!(err, IdlPackError::Checksum { .. }));
        // With the right checksum it is read, and the manifest is refused.
        let err = idls_from_pack(&packed, &sha256_bytes(&packed)).unwrap_err();
        assert!(matches!(err, IdlPackError::Manifest(_)));
    }

    #[test]
    fn a_manifest_entry_without_its_file_is_an_error() {
        let manifest = br#"{"idls":[{"file":"a.json","programId":"11111111111111111111111111111111","name":"a","networks":["devnet"],"source":{"kind":"hand"},"changes":""}]}"#;
        let packed = idl_pack::pack(&[(idl_pack::MANIFEST, manifest)]);
        assert_eq!(
            idls_from_pack(&packed, &sha256_bytes(&packed)),
            Err(IdlPackError::Missing("a.json".into()))
        );
    }

    /// Every file in the pack declares the program and name its manifest
    /// entry gives, and no two claim a program on the same network, with the
    /// core files or each other.
    #[test]
    fn every_shipped_idl_declares_its_listed_program() {
        let idls = shipped_idls();
        assert!(!idls.is_empty());
        for idl in &idls {
            let text = std::str::from_utf8(&idl.json).unwrap();
            let client =
                ProgramClient::from_json(text).unwrap_or_else(|err| panic!("{}: {err}", idl.name));
            assert_eq!(client.program_address(), idl.program_id, "{}", idl.name);
            assert_eq!(client.root().program.name.to_string(), idl.name);
            assert!(!idl.networks.is_empty(), "{} runs nowhere", idl.name);
            if let Some(source) = idl.source.chain() {
                assert_ne!(source.sha256, sha256_bytes(&idl.json), "{}", idl.name);
            }
        }
        let core = crate::idl::builtin::BUILTIN_IDLS
            .iter()
            .map(|idl| (idl.program_id, idl.networks.to_vec()));
        let all = core.chain(
            idls.iter()
                .map(|idl| (idl.program_id, idl.networks.clone())),
        );
        let mut seen = std::collections::BTreeSet::new();
        for (program_id, networks) in all {
            for network in networks {
                assert!(
                    seen.insert((network, program_id)),
                    "{program_id} is listed twice on {network}"
                );
            }
        }
    }

    /// The mainnet Raydium files are the devnet ones with the addresses of
    /// mainnet, which is all the two networks' IDLs differ in.
    #[test]
    fn the_mainnet_raydium_files_differ_from_devnet_by_addresses_alone() {
        let idls = shipped_idls();
        let json = |program: &str| {
            let program = Pubkey::from_str(program).unwrap();
            let idl = idls.iter().find(|idl| idl.program_id == program).unwrap();
            String::from_utf8(idl.json.to_vec()).unwrap()
        };
        const ADMIN: (&str, &str) = (
            "DRayqG9RXYi8WHgWEmRQGrUWRWbhjYWYkCRJDd6JBBak",
            "GThUX1Atko4tqhN2NaiTazWSeFWMuiUvfFnyJyUghFMJ",
        );
        /// Devnet program, mainnet program, and the other addresses swapped.
        type Pair<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);
        let pairs: [Pair; 2] = [
            (
                "DRayAUgENGQBKVaX8owNhgzkEDyoHTGVEGHVJT1E9pfH",
                "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK",
                &[ADMIN],
            ),
            (
                "DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb",
                "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
                &[
                    ADMIN,
                    (
                        "3oE58BKVt8KuYkGxx8zBojugnymWmBiyafWgMrnb6eYy",
                        "DNXgeM9EiiaAbaWvwjHj9fQQLAX5ZsfHyvmYUNRAdNC8",
                    ),
                ],
            ),
        ];
        for (devnet, mainnet, swaps) in pairs {
            let mut expected = json(devnet).replace(devnet, mainnet);
            for (from, to) in swaps {
                expected = expected.replace(from, to);
            }
            assert_eq!(json(mainnet), expected, "{mainnet}");
        }
    }
}
