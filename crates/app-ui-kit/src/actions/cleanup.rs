//! Burning and closing a wallet's token accounts, one step at a time.
//!
//! The modal reads the accounts ([`read`]) and plans them as the user picks
//! ([`plan`]). [`start`] puts the first step left on the Sign screen and
//! remembers the run. Once that step lands, Submit offers [`next`], which
//! reads the chain again and puts the step after it on the Sign screen, or
//! opens the modal again when nothing chosen is left.

use crate::state::forms::{CleanupRun, SignLoad, TxSource};
use crate::state::session::Session;
use dioxus::prelude::{ReadableExt, WritableExt};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;
use wallet_core::token_plan::{Choice, Held, TokenPlan};
use wallet_core::WalletId;

/// The token accounts of wallet `id`, read from the chain.
pub async fn read(session: Session, id: WalletId) -> Result<Vec<Held>, String> {
    let route = session.route_of(id).map_err(|err| err.to_string())?;
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    wallet_flows::cleanup::read(rpc.as_ref(), route.address)
        .await
        .map_err(|err| err.to_string())
}

/// What `chosen` does to `held`, for wallet `id`.
pub fn plan(
    session: Session,
    id: WalletId,
    held: Vec<Held>,
    chosen: &BTreeMap<Pubkey, Choice>,
) -> Result<TokenPlan, String> {
    let route = session.route_of(id).map_err(|err| err.to_string())?;
    wallet_flows::cleanup::plan(&route, held, chosen).map_err(|err| err.to_string())
}

/// Read wallet `id`'s accounts again, plan `chosen`, and put the first step
/// on the Sign screen, counted after `done` that landed. `Ok(false)` when
/// nothing chosen is left to do.
pub async fn start(
    session: Session,
    id: WalletId,
    chosen: BTreeMap<Pubkey, Choice>,
    done: usize,
) -> Result<bool, String> {
    let route = session.route_of(id).map_err(|err| err.to_string())?;
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    let held = wallet_flows::cleanup::read(rpc.as_ref(), route.address)
        .await
        .map_err(|err| err.to_string())?;
    let plan = wallet_flows::cleanup::plan(&route, held, &chosen).map_err(|err| err.to_string())?;
    if plan.steps.is_empty() {
        return Ok(false);
    }
    let (prepared, height) = wallet_flows::cleanup::prepare(rpc.as_ref(), &route, &plan, 0, done)
        .await
        .map_err(|err| err.to_string())?;
    session.open_in_sign(
        SignLoad::new(prepared.wire, TxSource::Wallets)
            .with_note(prepared.note)
            .expiring_at(height),
    );
    let form = session.forms.cleanup;
    let (mut run, mut open) = (form.run, form.open);
    run.set(Some(CleanupRun {
        wallet: id,
        chosen,
        done,
        flow: *session.forms.sign.flow.peek(),
    }));
    open.set(None);
    Ok(true)
}

/// The step of a cleanup that is on the Sign screen, as its number among
/// the steps so far, when there is one.
pub fn step_on_screen(session: &Session) -> Option<usize> {
    let run = session.forms.cleanup.run.read();
    let run = run.as_ref()?;
    let on_screen = *session.forms.sign.flow.read();
    (run.flow.is_some() && run.flow == on_screen).then_some(run.done + 1)
}

/// The step on the Sign screen landed: put the next one there, or open the
/// modal again to say it is done or what went wrong.
pub async fn next(session: Session) {
    let form = session.forms.cleanup;
    let Some(run) = form.run.peek().clone() else {
        return;
    };
    let landed = run.done + 1;
    let (mut runs, mut notice, mut open) = (form.run, form.notice, form.open);
    match start(session, run.wallet, run.chosen.clone(), landed).await {
        Ok(true) => {}
        Ok(false) => {
            runs.set(None);
            notice.set(Some(match landed {
                1 => "It landed. Nothing you chose is left to close.".into(),
                2 => "Both steps landed. Nothing you chose is left to close.".into(),
                _ => format!("All {landed} steps landed. Nothing you chose is left to close."),
            }));
            session.forms.sign.clear();
            open.set(Some(run.wallet));
        }
        Err(err) => {
            runs.set(Some(CleanupRun {
                done: landed,
                flow: None,
                ..run.clone()
            }));
            notice.set(Some(format!(
                "Step {landed} landed, but the next one could not be made: {err}"
            )));
            session.forms.sign.clear();
            open.set(Some(run.wallet));
        }
    }
}
