//! What the node says the transaction would do, and, for a transaction that
//! stores a Squads proposal it does not execute, what the proposal would do.

use super::*;
use app_ui_kit::ui::SimulationStrip as Strip;

/// One quiet line about what the transaction would do, with the details a
/// click away (`app_ui_kit::ui::SimulationStrip`).
#[component]
pub(in crate::ui) fn SimulationStrip(tx: std::rc::Rc<DecodedTx>, inspect: bool) -> Element {
    let session = use_context::<Session>();
    let state = session.forms.sign.simulation;
    let online = crate::state::use_network().is_some();
    let can_run = online && !matches!(state(), SimState::Running);
    let again = tx.clone();
    let on_again = can_run
        .then(|| EventHandler::new(move |_| start_simulation(session, again.as_ref().clone())));
    let proposal = session.forms.sign.proposal_simulation;

    rsx! {
        Strip { state, limit: tx.budget.compute_unit_limit, inspect, on_again }
        // What the vault would run, simulated beside the transaction that
        // stores it, and run again with it.
        if proposal() != SimState::Idle {
            div { class: "sign-status",
                h3 { "What the proposal would do" }
            }
            Strip { state: proposal, limit: None, inspect, changes: true }
        }
    }
}
