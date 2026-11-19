//! Keeping a Squads wallet's record in step with its multisig on chain.
//!
//! A multisig's members, threshold, time lock, config authority and rent
//! collector are read from the chain and kept on its record, which the
//! wallet's card, its Details, its Settings and the route a transaction takes
//! all read. They change on chain when the members vote a change, so they are
//! read again: for every multisig once the vault is unlocked and the node
//! answers (the online shell starts that), whenever the Proposals modal reads
//! one, and by hand from the wallet's Details.
//!
//! What the chain says is about the cluster the app is on. A multisig that is
//! not there keeps what its record has, and so does one the node could not be
//! asked about. The vault is written only when something changed.

use crate::state::session::{Session, WalletSummary};
use dioxus::prelude::ReadableExt;
use solana_pubkey::Pubkey;
use wallet_core::squads::{Squad, SquadChanges, SquadConfig, SquadState};
use wallet_core::{WalletId, WalletKind};
use wallet_flows::squads::Lookup;

/// The first of this device's wallets that is one of `members`.
pub(crate) fn local_member(wallets: &[WalletSummary], members: &[Pubkey]) -> Option<WalletId> {
    super::wallets::member_candidates(wallets)
        .into_iter()
        .find(|wallet| {
            wallet
                .address
                .is_some_and(|address| members.contains(&address))
        })
        .map(|wallet| wallet.id)
}

/// Keep `read` on `kind`, a multisig's record, as one function does for every
/// way a reading arrives. When the members were never read before and nobody
/// acts for it yet, the first wallet of this device that is a member is
/// chosen. A choice the person made is never replaced: a wallet that is no
/// longer a member stays chosen and is shown as such.
pub fn apply_reading(
    kind: &mut WalletKind,
    read: &SquadConfig,
    wallets: &[WalletSummary],
) -> SquadChanges {
    let changes = kind.set_squad_config(read);
    let unchosen = matches!(
        kind,
        WalletKind::Squads {
            member_wallet: None,
            ..
        }
    );
    if changes.first_read && unchosen {
        let keys: Vec<Pubkey> = read.members.iter().map(|member| member.key).collect();
        kind.set_member_wallet(local_member(wallets, &keys));
    }
    changes
}

/// Keep what was read of the multisig of wallet `id`. Writes the vault only
/// when the record changes, and only while it is unlocked. Answers what
/// changed, or `None` when the wallet is gone or is not a multisig.
pub fn keep_reading(session: Session, id: WalletId, state: &SquadState) -> Option<SquadChanges> {
    let wallets = session.wallet_summaries();
    let before = wallets
        .iter()
        .find(|wallet| wallet.id == id)?
        .contract
        .clone()
        .filter(|kind| kind.squad().is_some())?;
    let read = SquadConfig::from(state);
    let mut after = before.clone();
    let changes = apply_reading(&mut after, &read, &wallets);
    if after != before && session.is_unlocked() {
        let _ = session.update_wallet(id, |kind| {
            apply_reading(kind, &read, &wallets);
        });
    }
    Some(changes)
}

/// The multisigs among this device's wallets, or among `only` when given.
fn squads_of(session: Session, only: Option<&[WalletId]>) -> Vec<(WalletId, Squad)> {
    session
        .wallet_summaries()
        .into_iter()
        .filter(|wallet| only.is_none_or(|only| only.contains(&wallet.id)))
        .filter_map(|wallet| Some((wallet.id, wallet.contract.as_ref()?.squad()?)))
        .collect()
}

/// Read again every multisig among `only`, or all of them when `None`, in one
/// request, and keep what changed. Quiet: a node that does not answer, or a
/// multisig that is not on this cluster, changes nothing and says nothing.
pub async fn refresh_quietly(session: Session, only: Option<Vec<WalletId>>) {
    let squads = squads_of(session, only.as_deref());
    if squads.is_empty() || !session.is_unlocked() {
        return;
    }
    let Ok(rpc) = session.rpc() else {
        return;
    };
    let addresses: Vec<Squad> = squads.iter().map(|(_, squad)| *squad).collect();
    let found = wallet_flows::squads::look_up_all(Some(rpc.as_ref()), &addresses).await;
    for ((id, _), lookup) in squads.into_iter().zip(found) {
        if let Lookup::Found(state) = lookup {
            keep_reading(session, id, &state);
        }
    }
}

/// Read the multisig of wallet `id` again, by hand, and keep what changed.
/// Answers what to tell the person: what changed, or that nothing did. The
/// vault has to be unlocked, as for any change to a wallet.
pub async fn refresh_by_hand(session: Session, id: WalletId) -> Result<String, String> {
    let squad = squads_of(session, Some(&[id]))
        .first()
        .map(|(_, squad)| *squad)
        .ok_or_else(|| "That wallet is not a multisig on this device.".to_string())?;
    let rpc = session.rpc().ok();
    let words = session.cluster().network_key();
    let words = wallet_core::Cluster::network_words(&words);
    match wallet_flows::squads::look_up(rpc.as_deref(), &squad).await {
        Lookup::Found(state) => {
            let changes = keep_reading(session, id, &state)
                .ok_or_else(|| "That wallet is no longer on this device.".to_string())?;
            // A wallet or contact is called by its name, anyone else by
            // their address.
            let directory = session.directory.peek().clone();
            let name = |key: &Pubkey| {
                directory
                    .label_of(key)
                    .unwrap_or_else(|| wallet_core::short_pubkey(key))
            };
            let said = changes.sentences(squad.program, &SquadConfig::from(&state), &name);
            Ok(if said.is_empty() {
                format!("Read again from {words}. Nothing changed.")
            } else {
                said.join(" ")
            })
        }
        Lookup::NotAMultisig => Err(format!(
            "This multisig is not on {words}, so what is shown was kept as it was. It may be on another network: switch the network in Settings to read it there."
        )),
        Lookup::Unreadable => Err(format!(
            "The account on {words} belongs to the Squads program but does not read as a multisig, so what is shown was kept as it was."
        )),
        Lookup::Unreachable => Err(
            "The node did not answer, so what is shown was kept as it was. Try again once it does.".into(),
        ),
    }
}

#[cfg(test)]
mod tests;
