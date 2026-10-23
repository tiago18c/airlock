//! The online build's network chip in the sidebar's status bar: whether the node answered
//! and which network, with the cluster, the node and the background sync a
//! hover away.

use app_core::{JobSyncStatus, Reach};
use app_ui_kit::actions::unix_now;
use app_ui_kit::Session;
use dioxus::prelude::*;
use wallet_core::{Cluster, KnownCluster};

/// "Online · devnet": the network, never the provider or the name a custom
/// cluster was given. Hovering or focusing it shows those, the node's URL,
/// and what the background sync last did. The dot turns red when the sync failed.
#[component]
pub fn NetworkChip() -> Element {
    let session = use_context::<Session>();
    let network = (session.network)();
    let sync = (session.sync)();
    let job = sync.jobs.first();
    let reach = match network.reach {
        Reach::Probing => "Detecting",
        Reach::Answered => "Online",
        Reach::Unreachable => "Unreachable",
    };
    let sync_failed = job.is_some_and(|job| job.last_error.is_some());
    let class = if sync_failed {
        "status net-chip sync-err"
    } else {
        match network.reach {
            Reach::Probing => "status net-chip probing",
            Reach::Answered => "status net-chip online",
            Reach::Unreachable => "status net-chip offline",
        }
    };
    let kind = network_kind(&network.cluster);
    let cluster = network.cluster.name().to_string();
    let url = network.cluster.rpc_url().to_string();
    let sync_rows = job.map(|job| sync_view(job, unix_now()));

    rsx! {
        span {
            class: "{class}",
            tabindex: "0",
            aria_label: "{reach} on {kind}",
            span { class: "status-dot" }
            span {
                "{reach}"
                span { class: "net-cluster", " · {kind}" }
            }
            // Not a place to move the window from.
            div { class: "net-card", role: "tooltip", "data-drag": "false",
                dl {
                    dt { "Cluster" }
                    dd { "{cluster}" }
                    dt { "Node" }
                    dd { code { "{url}" } }
                    dt { "Status" }
                    dd { "{reach}" }
                    if let Some((sync, next)) = sync_rows {
                        dt { "Sync" }
                        dd { class: if sync_failed { "net-err" }, "{sync}" }
                        dt { "Next" }
                        dd { "{next}" }
                    }
                }
            }
        }
    }
}

/// Which network the node is: localnet when it runs on this machine,
/// otherwise the public cluster its genesis hash matched.
fn network_kind(cluster: &Cluster) -> &'static str {
    let known = match cluster {
        Cluster::MainnetBeta => Some(KnownCluster::MainnetBeta),
        Cluster::Devnet => Some(KnownCluster::Devnet),
        Cluster::Testnet => Some(KnownCluster::Testnet),
        Cluster::Custom { rpc_url, known, .. } => {
            if is_local(rpc_url) {
                return "localnet";
            }
            *known
        }
    };
    match known {
        Some(KnownCluster::MainnetBeta) => "mainnet",
        Some(KnownCluster::Devnet) => "devnet",
        Some(KnownCluster::Testnet) => "testnet",
        None => "custom",
    }
}

fn is_local(rpc_url: &str) -> bool {
    let rest = rpc_url
        .strip_prefix("https://")
        .or_else(|| rpc_url.strip_prefix("http://"))
        .unwrap_or(rpc_url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = if host.starts_with('[') {
        host.split(']').next().map(|h| &h[1..]).unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or_default()
    };
    host == "localhost" || host.starts_with("127.") || host == "::1" || host == "0.0.0.0"
}

/// What the sync last did, and when it runs next.
fn sync_view(job: &JobSyncStatus, now_secs: i64) -> (String, String) {
    let next = match job.next_run_ms {
        Some(ms) => {
            let wait = ((ms / 1000) as i64 - now_secs).max(0);
            if wait == 0 {
                "Now".into()
            } else if wait < 60 {
                format!("In {wait}s")
            } else {
                format!("In {}m", (wait + 59) / 60)
            }
        }
        None => "—".into(),
    };
    let last = if let Some(err) = job.last_error.as_deref() {
        format!("Failed: {}", err.trim_end_matches('.'))
    } else if let Some(ms) = job.last_success_ms {
        let when = app_ui_kit::ui::local_time::clock_text_seconds((ms / 1000) as i64);
        format!("Synced {when}")
    } else {
        "Pending".into()
    };
    (last, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(name: &str, url: &str, known: Option<KnownCluster>) -> Cluster {
        Cluster::Custom {
            name: name.into(),
            rpc_url: url.into(),
            known,
        }
    }

    #[test]
    fn a_provider_shows_as_its_network() {
        let helius = custom(
            "devnet helius",
            "https://devnet.helius-rpc.com/?api-key=k",
            Some(KnownCluster::Devnet),
        );
        assert_eq!(network_kind(&helius), "devnet");
        assert_eq!(network_kind(&Cluster::MainnetBeta), "mainnet");
    }

    #[test]
    fn a_node_on_this_machine_is_localnet() {
        for url in [
            "http://127.0.0.1:8899",
            "http://localhost:8899/",
            "http://[::1]:8899",
        ] {
            // A mainnet fork such as surfpool answers with mainnet's genesis.
            let local = custom("surfpool", url, Some(KnownCluster::MainnetBeta));
            assert_eq!(network_kind(&local), "localnet", "{url}");
        }
    }

    #[test]
    fn an_unrecognised_node_is_custom() {
        let other = custom("mine", "https://rpc.example.com", None);
        assert_eq!(network_kind(&other), "custom");
    }
}
