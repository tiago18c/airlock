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
