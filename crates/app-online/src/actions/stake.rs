//! Stake operations, from a checked form to the Sign screen.

use app_ui_kit::Session;
use app_ui_kit::TxSource;
use dioxus::prelude::ReadableExt;
use wallet_core::stake::STAKE_RENT_LAMPORTS;
use wallet_core::WalletId;
use wallet_flows::stake::StakeRequest;

/// Compile `request` for `wallet` on a fresh blockhash and open it in Sign.
pub async fn prepare(
    session: Session,
    wallet: WalletId,
    request: StakeRequest,
) -> Result<(), String> {
    let route = session.route_of(wallet).map_err(|err| err.to_string())?;
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    // What the cluster said a stake account has to keep, when it was asked.
    let rent = session
        .forms
        .stake
        .snapshot
        .peek()
        .as_ref()
        .map(|found| found.rent_reserve)
        .unwrap_or(STAKE_RENT_LAMPORTS);
    let prepared = wallet_flows::stake::prepare(rpc.as_ref(), &route, request, rent)
        .await
        .map_err(|err| err.to_string())?;
    session.open_in_sign(prepared.wire, TxSource::Stake, prepared.note);
    Ok(())
}
