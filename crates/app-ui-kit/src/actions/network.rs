//! UI-facing network helpers. The monitor that opens sockets lives in the
//! online shell and is plugged into [`Session`] at startup.

use crate::state::session::Session;
use dioxus::prelude::WritableExt;
use wallet_core::Cluster;

pub fn start_detect(session: Session) {
    session.start_detect();
}

/// Read what changed after a wallet or an address was added.
pub fn start_refresh(session: Session) {
    session.start_refresh();
}

/// The address book's Refresh button: run-now on the balances sync job.
/// Note and error wording match the pre-scheduler Refresh path.
pub async fn refresh_from_chain(session: Session) -> Result<String, String> {
    session.refresh_from_chain().await
}

/// In an app with no node: read transactions for the network of `cluster`
/// from now on. The IDL catalog follows it; there is no node to ask.
pub fn read_offline_for(session: Session, cluster: Cluster) {
    session.edit_workspace(|ws| ws.read_offline_for(cluster));
}

/// Switch to `cluster`, find out whether it answers, and read everything
/// again from it. What was read from the old
/// cluster is dropped first, so none of it is shown as this one's.
pub fn use_cluster(session: Session, cluster: Cluster) {
    let mut moved = false;
    session.edit_workspace(|ws| moved = ws.use_cluster(cluster));
    if moved {
        let stake = session.forms.stake;
        let (mut snapshot, mut loaded_for, mut error) =
            (stake.snapshot, stake.loaded_for, stake.error);
        snapshot.set(None);
        loaded_for.set(None);
        error.set(None);
        stake.clear_rewards();
    }
    start_detect(session);
}

#[cfg(test)]
mod tests {
    //! An app with no node: the network it reads transactions for, through
    //! the real session, on a window with no screen drawn.

    use super::*;
    use crate::state::session::OnlineHooks;
    use crate::state::submit::SubmitHandle;
    use crate::state::{settle_offline_network, use_session};
    use app_core::testing::kit;
    use app_core::Core;
    use dioxus::dioxus_core::{NoOpMutations, ScopeId, VirtualDom};
    use dioxus::prelude::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;
    use wallet_core::IdlNetwork;

    #[derive(Clone)]
    struct Held {
        core: Core,
        session: Rc<Cell<Option<Session>>>,
    }

    fn offline_app(held: Held) -> Element {
        let core = use_hook(|| CopyValue::new(held.core.clone()));
        use_hook(|| settle_offline_network(&core.peek()));
        let session = use_session(core, OnlineHooks::none(), SubmitHandle::none());
        held.session.set(Some(session));
        rsx! {}
    }

    async fn settle(dom: &mut VirtualDom) {
        for _ in 0..4 {
            let _ = tokio::time::timeout(Duration::from_millis(40), dom.wait_for_work()).await;
            dom.render_immediate(&mut NoOpMutations);
        }
    }

    fn open(core: &Core) -> (VirtualDom, Session) {
        let session = Rc::new(Cell::new(None));
        let mut dom = VirtualDom::new_with_props(
            offline_app,
            Held {
                core: core.clone(),
                session: session.clone(),
            },
        );
        dom.rebuild_in_place();
        let session = session.get().unwrap();
        (dom, session)
    }

    #[tokio::test]
    async fn an_offline_app_reads_mainnet_until_another_network_is_chosen() {
        let kit = kit();
        kit.core.create_vault("pw").unwrap();

        // A vault on the default cluster, devnet, which nobody chose.
        let (mut dom, session) = open(&kit.core);
        settle(&mut dom).await;
        assert_eq!(
            dom.in_scope(ScopeId::ROOT, || session.idl_network()),
            IdlNetwork::Mainnet
        );

        // Devnet chosen in Settings: the catalog follows, and stays there
        // when the app opens again.
        dom.in_scope(ScopeId::ROOT, || read_offline_for(session, Cluster::Devnet));
        settle(&mut dom).await;
        assert_eq!(
            dom.in_scope(ScopeId::ROOT, || session.idl_network()),
            IdlNetwork::Devnet
        );
        drop(dom);

        let (mut dom, session) = open(&kit.core);
        settle(&mut dom).await;
        assert_eq!(
            dom.in_scope(ScopeId::ROOT, || session.idl_network()),
            IdlNetwork::Devnet
        );
        assert_eq!(
            kit.core.with_workspace(|ws| ws.cluster.clone()),
            Cluster::Devnet
        );
    }
}
