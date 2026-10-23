//! The cluster in use, and the client that talks to it.

use super::*;

impl Session {
    pub fn cluster(&self) -> Cluster {
        (self.network)().cluster.clone()
    }

    /// The URL of the node the current cluster is read from.
    pub fn rpc_url(&self) -> String {
        (self.network)().cluster.rpc_url().to_string()
    }

    /// The client for the current cluster, shared by everything that asks
    /// the node. A shell with no network has none, so nothing opens a socket.
    pub fn rpc(&self) -> Result<Arc<dyn SolanaRpc>, RpcError> {
        (self.online.peek().get_rpc)()
    }
}
