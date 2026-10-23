//! Solana Explorer links. Custom RPC URLs never appear in a link unless the
//! host is loopback (no API key there).

use solana_pubkey::Pubkey;
use solana_signature::Signature;

use crate::cluster::{Cluster, KnownCluster};

const BASE: &str = "https://explorer.solana.com";

/// Explorer URL for a transaction on `cluster`, or `None` when the cluster's
/// RPC URL must not leave the app (custom with a query, or an unknown remote).
pub fn tx_url(signature: &Signature, cluster: &Cluster) -> Option<String> {
    link("tx", &signature.to_string(), cluster)
}

/// Explorer URL for an address on `cluster`, same rules as [`tx_url`].
pub fn address_url(pubkey: &Pubkey, cluster: &Cluster) -> Option<String> {
    link("address", &pubkey.to_string(), cluster)
}

fn link(kind: &str, id: &str, cluster: &Cluster) -> Option<String> {
    let path = format!("{BASE}/{kind}/{id}");
    match cluster {
        Cluster::MainnetBeta => Some(path),
        Cluster::Devnet => Some(format!("{path}?cluster=devnet")),
        Cluster::Testnet => Some(format!("{path}?cluster=testnet")),
        Cluster::Custom { rpc_url, known, .. } => custom_link(&path, rpc_url, *known),
    }
}

fn custom_link(path: &str, rpc_url: &str, known: Option<KnownCluster>) -> Option<String> {
    // Local validators: Solana Explorer can talk to them via customUrl.
    // Loopback wins over a known genesis (a local fork is not on the public
    // explorer). A query on a loopback URL is refused rather than forwarded.
    if is_loopback(rpc_url) {
        if rpc_url.contains('?') {
            return None;
        }
        return Some(format!(
            "{path}?cluster=custom&customUrl={}",
            percent_encode(rpc_url)
        ));
    }
    // A remote custom RPC that shares a public genesis: use that network's
    // link so the RPC URL (and any key in its query) never leaves the app.
    if let Some(known) = known {
        return link_for_known(path, known);
    }
    // Anything else: the URL may carry an API key, so no link.
    if rpc_url.contains('?') {
        return None;
    }
    None
}

fn link_for_known(path: &str, known: KnownCluster) -> Option<String> {
    match known {
        KnownCluster::MainnetBeta => Some(path.to_string()),
        KnownCluster::Devnet => Some(format!("{path}?cluster=devnet")),
        KnownCluster::Testnet => Some(format!("{path}?cluster=testnet")),
    }
}

fn is_loopback(rpc_url: &str) -> bool {
    let rest = rpc_url
        .strip_prefix("https://")
        .or_else(|| rpc_url.strip_prefix("http://"))
        .unwrap_or(rpc_url);
    let host = rest.split(['/', '?', '#', ':']).next().unwrap_or_default();
    host == "localhost" || host == "127.0.0.1"
}

/// Encode for a query value. Unreserved characters stay as-is.
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{KnownCluster, LOCAL_RPC_URL, MAINNET_BETA_GENESIS};
    use crate::Cluster;
    use solana_hash::Hash;
    use std::str::FromStr;

    fn sig() -> Signature {
        Signature::from([7u8; 64])
    }

    fn sig_str() -> String {
        sig().to_string()
    }

    #[test]
    fn mainnet_beta_has_no_cluster_query() {
        assert_eq!(
            tx_url(&sig(), &Cluster::MainnetBeta),
            Some(format!("{BASE}/tx/{}", sig_str()))
        );
    }

    #[test]
    fn devnet_and_testnet_set_cluster() {
        assert_eq!(
            tx_url(&sig(), &Cluster::Devnet),
            Some(format!("{BASE}/tx/{}?cluster=devnet", sig_str()))
        );
        assert_eq!(
            tx_url(&sig(), &Cluster::Testnet),
            Some(format!("{BASE}/tx/{}?cluster=testnet", sig_str()))
        );
    }

    #[test]
    fn custom_with_known_genesis_uses_that_cluster_without_rpc_url() {
        let custom = Cluster::Custom {
            name: "helius".into(),
            rpc_url: "https://mainnet.helius-rpc.com".into(),
            known: Some(KnownCluster::MainnetBeta),
        };
        let url = tx_url(&sig(), &custom).unwrap();
        assert_eq!(url, format!("{BASE}/tx/{}", sig_str()));
        assert!(!url.contains("helius"));
        assert!(!url.contains("customUrl"));

        // Same outcome after reconcile marks the genesis.
        let reconciled = Cluster::custom("helius", "https://mainnet.helius-rpc.com")
            .unwrap()
            .reconcile_genesis(&Hash::from_str(MAINNET_BETA_GENESIS).unwrap());
        assert_eq!(
            tx_url(&sig(), &reconciled),
            tx_url(&sig(), &Cluster::MainnetBeta)
        );
    }

    #[test]
    fn custom_localhost_uses_custom_url() {
        let local = Cluster::custom("surfpool", LOCAL_RPC_URL).unwrap();
        assert_eq!(
            tx_url(&sig(), &local),
            Some(format!(
                "{BASE}/tx/{}?cluster=custom&customUrl={}",
                sig_str(),
                percent_encode(LOCAL_RPC_URL)
            ))
        );
        // Loopback wins over a known genesis (local fork).
        let fork = local.reconcile_genesis(&Hash::from_str(MAINNET_BETA_GENESIS).unwrap());
        let url = tx_url(&sig(), &fork).unwrap();
        assert!(url.contains("customUrl"));
        assert!(!url.contains("helius"));
    }

    #[test]
    fn custom_unknown_remote_is_none() {
        let custom = Cluster::custom("mine", "https://rpc.example.com").unwrap();
        assert_eq!(tx_url(&sig(), &custom), None);
    }

    #[test]
    fn custom_url_with_a_query_is_none() {
        let custom = Cluster::custom("helius", "https://rpc.example.com/?api-key=SECRET").unwrap();
        assert_eq!(tx_url(&sig(), &custom), None);
        // With a known genesis the public link is used; the keyed URL stays out.
        let with_known = Cluster::Custom {
            name: "helius".into(),
            rpc_url: "https://rpc.example.com/?api-key=SECRET".into(),
            known: Some(KnownCluster::MainnetBeta),
        };
        let url = tx_url(&sig(), &with_known).unwrap();
        assert_eq!(url, format!("{BASE}/tx/{}", sig_str()));
        assert!(!url.contains("SECRET"));
        assert!(!url.contains("api-key"));
    }

    #[test]
    fn address_url_follows_the_same_rules() {
        let pk = Pubkey::new_from_array([3; 32]);
        assert_eq!(
            address_url(&pk, &Cluster::Devnet),
            Some(format!("{BASE}/address/{pk}?cluster=devnet"))
        );
    }
}
