use serde::{Deserialize, Serialize};
use solana_hash::Hash;

/// `getGenesisHash` for mainnet-beta.
pub const MAINNET_BETA_GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";
/// `getGenesisHash` for devnet.
pub const DEVNET_GENESIS: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";
/// `getGenesisHash` for testnet.
pub const TESTNET_GENESIS: &str = "4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpja8HXCS";

/// A public Solana cluster, used when a custom RPC shares its genesis hash so
/// explorer links can point at that network without putting the RPC URL in the
/// link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KnownCluster {
    MainnetBeta,
    Devnet,
    Testnet,
}

impl KnownCluster {
    pub fn from_genesis_hash(hash: &Hash) -> Option<Self> {
        match hash.to_string().as_str() {
            MAINNET_BETA_GENESIS => Some(Self::MainnetBeta),
            DEVNET_GENESIS => Some(Self::Devnet),
            TESTNET_GENESIS => Some(Self::Testnet),
            _ => None,
        }
    }

    pub fn cluster(self) -> Cluster {
        match self {
            Self::MainnetBeta => Cluster::MainnetBeta,
            Self::Devnet => Cluster::Devnet,
            Self::Testnet => Cluster::Testnet,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[derive(Default)]
pub enum Cluster {
    MainnetBeta,
    #[default]
    Devnet,
    Testnet,
    Custom {
        name: String,
        rpc_url: String,
        /// Set when the node answered with a well-known genesis. Explorer links
        /// use this so the RPC URL (and any key in it) never leave the app.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        known: Option<KnownCluster>,
    },
}

/// Where a validator started on this machine listens, such as
/// `solana-test-validator` or surfpool.
pub const LOCAL_RPC_URL: &str = "http://127.0.0.1:8899";

impl Cluster {
    /// A cluster reached through an RPC URL the user typed. The URL has to
    /// be `http://` or `https://` with a host. A blank name becomes "custom".
    pub fn custom(name: &str, rpc_url: &str) -> Result<Self, String> {
        let rpc_url = rpc_url.trim();
        let rest = rpc_url
            .strip_prefix("https://")
            .or_else(|| rpc_url.strip_prefix("http://"))
            .ok_or("The RPC URL has to start with http:// or https://.")?;
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if host.is_empty() || rpc_url.chars().any(char::is_whitespace) {
            return Err("The RPC URL needs a host, such as http://127.0.0.1:8899.".into());
        }
        let name = name.trim();
        Ok(Self::Custom {
            name: if name.is_empty() { "custom" } else { name }.to_string(),
            rpc_url: rpc_url.to_string(),
            known: None,
        })
    }

    pub fn is_custom(&self) -> bool {
        matches!(self, Self::Custom { .. })
    }

    pub fn rpc_url(&self) -> &str {
        match self {
            Self::MainnetBeta => "https://api.mainnet-beta.solana.com",
            Self::Devnet => "https://api.devnet.solana.com",
            Self::Testnet => "https://api.testnet.solana.com",
            Self::Custom { rpc_url, .. } => rpc_url,
        }
    }

    /// Where the node takes WebSocket subscriptions: the RPC URL on `ws://`
    /// or `wss://`. A port written in the URL is one higher, as a validator
    /// serves its WebSocket next to its RPC port (8899 and 8900).
    pub fn ws_url(&self) -> String {
        ws_url_for(self.rpc_url())
    }

    pub fn name(&self) -> &str {
        match self {
            Self::MainnetBeta => "mainnet-beta",
            Self::Devnet => "devnet",
            Self::Testnet => "testnet",
            Self::Custom { name, .. } => name,
        }
    }

    pub fn from_genesis_hash(hash: &Hash) -> Option<Self> {
        KnownCluster::from_genesis_hash(hash).map(KnownCluster::cluster)
    }

    /// Map a live genesis hash onto this cluster without dropping a custom RPC URL.
    pub fn reconcile_genesis(&self, hash: &Hash) -> Self {
        match (self, KnownCluster::from_genesis_hash(hash)) {
            (Self::Custom { name, rpc_url, .. }, known) => Self::Custom {
                name: name.clone(),
                rpc_url: rpc_url.clone(),
                known,
            },
            (_, Some(known)) => known.cluster(),
            (_, None) => Self::Custom {
                name: "custom".into(),
                rpc_url: self.rpc_url().to_string(),
                known: None,
            },
        }
    }
}

/// [`Cluster::ws_url`] for any RPC URL. The path and query stay, so a key
/// carried in either reaches the WebSocket too.
fn ws_url_for(rpc_url: &str) -> String {
    let (scheme, rest) = if let Some(rest) = rpc_url.strip_prefix("https://") {
        ("wss://", rest)
    } else if let Some(rest) = rpc_url.strip_prefix("http://") {
        ("ws://", rest)
    } else {
        return rpc_url.to_string();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    // A colon after the last `]` (an IPv6 host) and any user info starts a port.
    let host_start = authority.rfind(['@', ']']).map_or(0, |at| at + 1);
    let port = authority[host_start..]
        .rfind(':')
        .map(|colon| host_start + colon)
        .and_then(|colon| Some((colon, authority[colon + 1..].parse::<u16>().ok()?)));
    match port {
        Some((colon, port)) if port < u16::MAX => {
            format!("{scheme}{}:{}{tail}", &authority[..colon], port + 1)
        }
        _ => format!("{scheme}{rest}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn the_websocket_is_next_to_the_rpc_url() {
        assert_eq!(
            Cluster::MainnetBeta.ws_url(),
            "wss://api.mainnet-beta.solana.com"
        );
        assert_eq!(ws_url_for("http://127.0.0.1:8899"), "ws://127.0.0.1:8900");
        assert_eq!(
            ws_url_for("https://rpc.example.com/v1?api-key=abc"),
            "wss://rpc.example.com/v1?api-key=abc"
        );
        assert_eq!(
            ws_url_for("https://user:pw@rpc.example.com:8443/x"),
            "wss://user:pw@rpc.example.com:8444/x"
        );
        assert_eq!(ws_url_for("http://[::1]:8899/"), "ws://[::1]:8900/");
        assert_eq!(ws_url_for("http://[::1]/"), "ws://[::1]/");
    }

    fn hash(s: &str) -> Hash {
        Hash::from_str(s).expect("genesis hash")
    }

    #[test]
    fn genesis_hash_identifies_public_clusters() {
        assert_eq!(
            Cluster::from_genesis_hash(&hash(MAINNET_BETA_GENESIS)),
            Some(Cluster::MainnetBeta)
        );
        assert_eq!(
            Cluster::from_genesis_hash(&hash(DEVNET_GENESIS)),
            Some(Cluster::Devnet)
        );
        assert_eq!(
            Cluster::from_genesis_hash(&hash(TESTNET_GENESIS)),
            Some(Cluster::Testnet)
        );
        assert_eq!(
            Cluster::from_genesis_hash(&hash("11111111111111111111111111111111")),
            None
        );
    }

    #[test]
    fn a_custom_cluster_takes_an_http_url_with_a_host() {
        assert_eq!(
            Cluster::custom(" surfpool ", "  http://127.0.0.1:8899 "),
            Ok(Cluster::Custom {
                name: "surfpool".into(),
                rpc_url: LOCAL_RPC_URL.into(),
                known: None,
            })
        );
        let named = Cluster::custom("", "https://rpc.example/?api-key=abc").unwrap();
        assert_eq!(named.name(), "custom");
        assert_eq!(named.rpc_url(), "https://rpc.example/?api-key=abc");
        assert!(named.is_custom() && !Cluster::Devnet.is_custom());
        for bad in [
            "",
            "127.0.0.1:8899",
            "ws://127.0.0.1:8900",
            "http://",
            "https:///path",
            "http://local host:8899",
        ] {
            assert!(Cluster::custom("x", bad).is_err(), "{bad}");
        }
    }

    /// A local fork answers with the genesis hash of the network it forked.
    /// The typed URL still wins, so requests keep going to the fork.
    #[test]
    fn a_fork_of_a_public_cluster_stays_on_its_own_url() {
        let fork = Cluster::custom("surfpool", LOCAL_RPC_URL).unwrap();
        let reconciled = fork.reconcile_genesis(&hash(MAINNET_BETA_GENESIS));
        assert_eq!(reconciled.rpc_url(), LOCAL_RPC_URL);
        match reconciled {
            Cluster::Custom {
                known: Some(KnownCluster::MainnetBeta),
                ..
            } => {}
            other => panic!("expected known mainnet, got {other:?}"),
        }
    }

    #[test]
    fn reconcile_keeps_custom_url_and_corrects_well_known() {
        let custom = Cluster::Custom {
            name: "helius".into(),
            rpc_url: "https://rpc.example".into(),
            known: None,
        };
        assert_eq!(
            custom.reconcile_genesis(&hash(MAINNET_BETA_GENESIS)),
            Cluster::Custom {
                name: "helius".into(),
                rpc_url: "https://rpc.example".into(),
                known: Some(KnownCluster::MainnetBeta),
            }
        );
        assert_eq!(
            Cluster::MainnetBeta.reconcile_genesis(&hash(DEVNET_GENESIS)),
            Cluster::Devnet
        );
        let unknown = Cluster::Devnet.reconcile_genesis(&hash("11111111111111111111111111111111"));
        match unknown {
            Cluster::Custom {
                name,
                rpc_url,
                known: None,
            } => {
                assert_eq!(name, "custom");
                assert_eq!(rpc_url, Cluster::Devnet.rpc_url());
            }
            other => panic!("expected custom, got {other:?}"),
        }
    }
}
