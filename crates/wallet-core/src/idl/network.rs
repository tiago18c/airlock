//! The network an IDL is kept for. A program can run at one address on
//! mainnet and another on devnet, and the same address can run different
//! versions on each, so the catalog keeps one set of IDLs per network.

use crate::cluster::{Cluster, KnownCluster};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IdlNetwork {
    Mainnet,
    Devnet,
    Testnet,
    /// A node whose genesis is none of the public clusters, such as
    /// `solana-test-validator`. A fork of a public cluster reports that
    /// cluster's genesis, and reads its IDLs.
    Localnet,
}

impl IdlNetwork {
    pub const ALL: [IdlNetwork; 4] = [Self::Mainnet, Self::Devnet, Self::Testnet, Self::Localnet];

    /// The IDLs a cluster reads. A custom RPC reads those of the public
    /// cluster its genesis matched.
    pub fn of(cluster: &Cluster) -> Self {
        match cluster {
            Cluster::MainnetBeta => Self::Mainnet,
            Cluster::Devnet => Self::Devnet,
            Cluster::Testnet => Self::Testnet,
            Cluster::Custom { known, .. } => match known {
                Some(KnownCluster::MainnetBeta) => Self::Mainnet,
                Some(KnownCluster::Devnet) => Self::Devnet,
                Some(KnownCluster::Testnet) => Self::Testnet,
                None => Self::Localnet,
            },
        }
    }

    /// Lower case, used in file names and storage keys.
    pub fn key(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Devnet => "devnet",
            Self::Testnet => "testnet",
            Self::Localnet => "localnet",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|network| network.key() == key)
    }
}

impl std::fmt::Display for IdlNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_rpc_reads_the_idls_of_the_cluster_its_genesis_matched() {
        let fork = Cluster::Custom {
            name: "surfpool".into(),
            rpc_url: "http://127.0.0.1:8899".into(),
            known: Some(KnownCluster::MainnetBeta),
        };
        assert_eq!(IdlNetwork::of(&fork), IdlNetwork::Mainnet);
        let local = Cluster::custom("local", "http://127.0.0.1:8899").unwrap();
        assert_eq!(IdlNetwork::of(&local), IdlNetwork::Localnet);
        assert_eq!(IdlNetwork::of(&Cluster::Devnet), IdlNetwork::Devnet);
    }

    #[test]
    fn keys_round_trip() {
        for network in IdlNetwork::ALL {
            assert_eq!(IdlNetwork::from_key(network.key()), Some(network));
        }
    }
}
