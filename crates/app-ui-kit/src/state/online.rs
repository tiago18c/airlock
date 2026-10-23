//! Optional network capability for panes that have an online form.

use std::sync::Arc;

use dioxus::prelude::*;
use wallet_core::{RpcError, SolanaRpc};

use crate::state::session::Session;
use crate::state::slices::NetworkStatus;

/// Access to the node: status and an RPC client. Provided by the online shell.
#[derive(Clone, Copy)]
pub struct Network {
    session: Session,
}

impl Network {
    pub fn new(session: Session) -> Self {
        Self { session }
    }

    pub fn status(&self) -> Arc<NetworkStatus> {
        (self.session.network)()
    }

    pub fn rpc(&self) -> Result<Arc<dyn SolanaRpc>, RpcError> {
        self.session.rpc()
    }
}

/// The network capability, when this shell provides one.
pub fn use_network() -> Option<Network> {
    use_context::<Option<Network>>()
}
