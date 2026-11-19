//! Reading every Squads multisig again once the app starts.
//!
//! Each multisig's members, threshold and the rest of its configuration are
//! kept on its wallet record, which only the window can write: the vault is
//! out of reach of the sync jobs (see [`app_core::PublicCore`]). So this is a
//! hook of the window, not a sync job, and it runs once per unlock, as soon
//! as the node has answered a probe: when the app starts, the vault is
//! unlocked once, so that is once per start. A later lock and unlock reads
//! them again, as the balances are read again after an unlock. The periodic
//! refreshes and a change of cluster do not; Details reads one again by hand,
//! and the Proposals modal whenever it opens.
//!
//! Only the online shells call it. The offline app does not link this crate,
//! so it never asks a node.

use app_core::{Reach, VaultState};
use app_ui_kit::Session;
use dioxus::prelude::*;

/// Read every multisig again once the vault is unlocked and the node has
/// answered, once per unlock. Quiet: see
/// [`app_ui_kit::actions::squads::refresh_quietly`]. Call once, from the
/// root, after [`app_ui_kit::use_session`].
pub fn use_squads_refresh(session: Session) {
    // Whether this unlock has had its reading.
    let mut done = use_signal(|| false);
    use_effect(move || {
        let unlocked = (session.vault_state)() == VaultState::Unlocked;
        let answered = session.network.read().reach == Reach::Answered;
        if !unlocked {
            if *done.peek() {
                done.set(false);
            }
            return;
        }
        if answered && !*done.peek() {
            done.set(true);
            spawn(app_ui_kit::actions::squads::refresh_quietly(session, None));
        }
    });
}

#[cfg(test)]
mod tests {
    //! The hook on a window with no screen drawn, over a node that counts
    //! what it is asked.

    use super::*;
    use app_core::testing::kit;
    use app_core::Core;
    use app_ui_kit::{OnlineHooks, SubmitHandle};
    use dioxus::dioxus_core::{NoOpMutations, VirtualDom};
    use std::sync::Arc;
    use std::time::Duration;
    use wallet_core::squads::Squad;
    use wallet_core::{SolanaRpc, WalletRecord};
    use wallet_flows::testing::{pk, FakeRpc};

    #[derive(Clone)]
    struct Held {
        core: Core,
        rpc: Arc<FakeRpc>,
    }

    fn app(held: Held) -> Element {
        let core = use_hook(|| CopyValue::new(held.core.clone()));
        let rpc = held.rpc.clone();
        let hooks = OnlineHooks::new(
            move || Ok(rpc.clone() as Arc<dyn SolanaRpc>),
            || {},
            || {},
            || Box::pin(async { Ok(String::new()) }),
        );
        let session = app_ui_kit::use_session(core, hooks, SubmitHandle::none());
        use_squads_refresh(session);
        rsx! {}
    }

    async fn settle(dom: &mut VirtualDom) {
        for _ in 0..4 {
            let _ = tokio::time::timeout(Duration::from_millis(40), dom.wait_for_work()).await;
            dom.render_immediate(&mut NoOpMutations);
        }
    }

    #[tokio::test]
    async fn every_multisig_is_read_once_per_unlock_once_the_node_answers() {
        let kit = kit();
        kit.core.create_vault("pw").unwrap();
        let squad = Squad::v4(&pk(40));
        kit.core
            .enroll(
                "add_contract_wallet",
                |data| data.add_contract_wallet(WalletRecord::squads("Team", squad, vec![], 0)),
                |_, _| {},
            )
            .unwrap();
        let rpc = Arc::new(FakeRpc::default());
        let mut dom = VirtualDom::new_with_props(
            app,
            Held {
                core: kit.core.clone(),
                rpc: rpc.clone(),
            },
        );
        dom.rebuild_in_place();
        settle(&mut dom).await;
        // Unlocked, and no probe has answered yet: nothing is asked.
        assert_eq!(rpc.asked(), 0);

        kit.core.public().set_reach(Reach::Answered);
        settle(&mut dom).await;
        assert_eq!(rpc.asked(), 1, "one request for every multisig");

        // Probed again, or another cluster: not again.
        kit.core.public().set_reach(Reach::Probing);
        settle(&mut dom).await;
        kit.core.public().set_reach(Reach::Answered);
        settle(&mut dom).await;
        assert_eq!(rpc.asked(), 1);

        // Locked, then unlocked: once more.
        kit.core.lock();
        settle(&mut dom).await;
        assert_eq!(rpc.asked(), 1);
        kit.core.unlock("pw").unwrap();
        settle(&mut dom).await;
        assert_eq!(rpc.asked(), 2);
    }
}
