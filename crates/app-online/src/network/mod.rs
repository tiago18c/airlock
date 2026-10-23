//! The network monitor: probes the cluster and keeps a shared `wallet-rpc` client.

mod monitor;

pub use monitor::{rpc_for_cluster, Monitor};
