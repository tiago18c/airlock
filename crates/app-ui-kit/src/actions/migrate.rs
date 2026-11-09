//! Moving everything a wallet holds to another address, one step at a time.
//!
//! The modal checks the destination ([`check`]), reads what the wallet
//! holds ([`read`]) and plans it as the user picks ([`plan`]). [`start`]
//! puts the first step left on the Sign screen and remembers the run. Once
//! that step lands, Submit offers [`next`], which reads the wallet again
//! and puts the step after it on the Sign screen. When the read finds the
//! step did not run (a multisig proposal still waiting for approvals), or
//! nothing is left, the modal opens again to say so.

use crate::state::forms::{MigrateRun, SignLoad, TxSource};
use crate::state::session::Session;
use dioxus::prelude::{ReadableExt, WritableExt};
use solana_pubkey::Pubkey;
use wallet_core::{WalletClass, WalletId, WalletKind};
use wallet_flows::migrate::{Choices, DestinationCheck, MigrationPlan, Snapshot};

/// Whether `to` can receive what wallet `from` holds.
pub async fn check(
    session: Session,
    from: WalletId,
    to: Pubkey,
) -> Result<DestinationCheck, String> {
    let route = session.route_of(from).map_err(|err| err.to_string())?;
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    let known = is_wallet(&session, to);
    wallet_flows::migrate::check_destination(rpc.as_ref(), route.address, to, known)
        .await
        .map_err(|err| err.to_string())
}

/// Whether `address` is a wallet of this vault.
pub fn is_wallet(session: &Session, address: Pubkey) -> bool {
    session
        .wallet_summaries()
        .iter()
        .any(|wallet| wallet.address == Some(address))
}

/// What wallet `from` holds, for a move to `to`.
pub async fn read(session: Session, from: WalletId, to: Pubkey) -> Result<Snapshot, String> {
    let route = session.route_of(from).map_err(|err| err.to_string())?;
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    wallet_flows::migrate::read(rpc.as_ref(), &route, to, super::unix_now())
        .await
        .map_err(|err| err.to_string())
}

/// What `choices` do, for wallet `from`.
pub fn plan(
    session: Session,
    from: WalletId,
    snapshot: &Snapshot,
    choices: &Choices,
) -> Result<MigrationPlan, String> {
    let route = session.route_of(from).map_err(|err| err.to_string())?;
    wallet_flows::migrate::plan(&route, snapshot, choices).map_err(|err| err.to_string())
}

/// What the move starts with for `snapshot`.
pub fn default_choices(session: Session, from: WalletId, snapshot: &Snapshot) -> Choices {
    let flagged = snapshot
        .tokens
        .iter()
        .filter(|held| session.flag_of(&held.address) == Some(wallet_core::TokenFlag::Red))
        .map(|held| held.address)
        .collect();
    match session.route_of(from) {
        Ok(route) => wallet_flows::migrate::default_choices(&route, snapshot, &flagged),
        Err(_) => Choices::default(),
    }
}

/// What came of [`start`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Started {
    /// The next step is on the Sign screen.
    OnSign,
    /// Nothing chosen is left to do.
    Done,
}

/// Read wallet `from` again, plan `choices`, and put the next step on the
/// Sign screen, counted after `done` that landed.
pub async fn start(
    session: Session,
    from: WalletId,
    to: Pubkey,
    choices: Choices,
    done: usize,
) -> Result<Started, String> {
    let route = session.route_of(from).map_err(|err| err.to_string())?;
    let rpc = session.rpc().map_err(|err| err.to_string())?;
    let snapshot = wallet_flows::migrate::read(rpc.as_ref(), &route, to, super::unix_now())
        .await
        .map_err(|err| err.to_string())?;
    let plan =
        wallet_flows::migrate::plan(&route, &snapshot, &choices).map_err(|err| err.to_string())?;
    let left = plan.steps();
    let Some((prepared, height)) =
        wallet_flows::migrate::prepare(rpc.as_ref(), &route, &snapshot, &plan, done)
            .await
            .map_err(|err| err.to_string())?
    else {
        return Ok(Started::Done);
    };
    session.open_in_sign(
        SignLoad::new(prepared.wire, TxSource::Wallets)
            .with_note(prepared.note)
            .expiring_at(height),
    );
    let form = session.forms.migrate;
    let (mut run, mut open) = (form.run, form.open);
    run.set(Some(MigrateRun {
        from,
        to,
        choices,
        done,
        flow: *session.forms.sign.flow.peek(),
        left,
        waiting: false,
    }));
    open.set(None);
    Ok(Started::OnSign)
}

/// The step of a move that is on the Sign screen, as its number among the
/// steps so far, when there is one.
pub fn step_on_screen(session: &Session) -> Option<usize> {
    let run = session.forms.migrate.run.read();
    let run = run.as_ref()?;
    let on_screen = *session.forms.sign.flow.read();
    (run.flow.is_some() && run.flow == on_screen).then_some(run.done + 1)
}

/// The step on the Sign screen landed: put the next one there, or open the
/// modal again to say it is done, waits on approvals, or what went wrong.
pub async fn next(session: Session) {
    let form = session.forms.migrate;
    let Some(run) = form.run.peek().clone() else {
        return;
    };
    let landed = run.done + 1;
    // Did it run? A proposal that waits for approvals changed nothing yet.
    if still_to_run(session, &run).await {
        reopen(
            session,
            Some(MigrateRun {
                done: landed,
                flow: None,
                waiting: true,
                ..run.clone()
            }),
            "It landed, but what it proposes has not run yet: the multisig's other members still have to approve it on Proposals. Check again once it has executed.".into(),
        );
        return;
    }
    match start(session, run.from, run.to, run.choices.clone(), landed).await {
        Ok(Started::OnSign) => {}
        Ok(Started::Done) => {
            let mut finished = form.finished;
            finished.set(Some((run.from, run.to)));
            reopen(
                session,
                None,
                match landed {
                    1 => "It landed. Everything you chose has moved.".into(),
                    2 => "Both steps landed. Everything you chose has moved.".into(),
                    _ => format!("All {landed} steps landed. Everything you chose has moved."),
                },
            )
        }
        Err(err) => reopen(
            session,
            Some(MigrateRun {
                done: landed,
                flow: None,
                ..run.clone()
            }),
            format!("Step {landed} landed, but the next one could not be made: {err}"),
        ),
    }
}

/// Go on with a run the modal shows: once a proposal it waited on has run,
/// the next step goes on the Sign screen. `Ok(Some(why))` while it still
/// waits, so the same proposal is never made twice.
pub async fn resume(session: Session) -> Result<Option<String>, String> {
    let Some(run) = session.forms.migrate.run.peek().clone() else {
        return Ok(None);
    };
    if run.waiting && still_to_run(session, &run).await {
        return Ok(Some(
            "The proposal has not executed yet. Approve and execute it on Proposals, then check again."
                .into(),
        ));
    }
    match start(session, run.from, run.to, run.choices.clone(), run.done).await? {
        Started::OnSign => Ok(None),
        Started::Done => {
            let (mut runs, mut finished) =
                (session.forms.migrate.run, session.forms.migrate.finished);
            runs.set(None);
            finished.set(Some((run.from, run.to)));
            Ok(Some("Everything you chose has moved.".into()))
        }
    }
}

/// Whether the wallet, read again, still has as many steps left as when
/// `run`'s step was built.
async fn still_to_run(session: Session, run: &MigrateRun) -> bool {
    let (Ok(route), Ok(rpc)) = (session.route_of(run.from), session.rpc()) else {
        return false;
    };
    match wallet_flows::migrate::read(rpc.as_ref(), &route, run.to, super::unix_now()).await {
        Ok(snapshot) => wallet_flows::migrate::plan(&route, &snapshot, &run.choices)
            .is_ok_and(|plan| plan.steps() >= run.left),
        Err(_) => false,
    }
}

/// Clear the Sign screen and open the modal again on the run's wallet, with
/// `kept` as the run and `notice` at the top.
fn reopen(session: Session, kept: Option<MigrateRun>, notice: String) {
    let form = session.forms.migrate;
    let (mut run, mut said, mut open) = (form.run, form.notice, form.open);
    let from = kept
        .as_ref()
        .map(|run| run.from)
        .or_else(|| run.peek().as_ref().map(|run| run.from));
    run.set(kept);
    said.set(Some(notice));
    session.forms.sign.clear();
    open.set(Some(from));
}

/// What on this device still names wallet `old`, once its holdings moved:
/// wallets it pays fees for, multisigs it acts or approves in, programmatic
/// signers it is the authority of.
pub fn references(session: &Session, old: WalletId) -> Vec<String> {
    let wallets = session.wallet_summaries();
    let Some(address) = wallets
        .iter()
        .find(|wallet| wallet.id == old)
        .and_then(|wallet| wallet.address)
    else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for wallet in wallets.iter().filter(|wallet| wallet.id != old) {
        if wallet.fee_payer == Some(old) {
            found.push(format!("It pays the fees of {}.", wallet.label));
        }
        match &wallet.contract {
            Some(WalletKind::Squads {
                member_wallet,
                approves_with,
                ..
            }) => {
                if *member_wallet == Some(old) {
                    found.push(format!("It acts as the member of {}.", wallet.label));
                } else if approves_with.contains(&old) {
                    found.push(format!("It approves together in {}.", wallet.label));
                }
            }
            Some(kind) if kind.signer_authority() == Some(address) => {
                found.push(format!(
                    "It is the authority of the programmatic signer {}.",
                    wallet.label
                ));
            }
            _ => {}
        }
    }
    if wallets
        .iter()
        .any(|wallet| wallet.id == old && wallet.class == WalletClass::Offline)
    {
        // Its key is on another device, which keeps it in its own vault.
        found.push("Its key is still in the vault of the device that signs for it.".into());
    }
    found
}
