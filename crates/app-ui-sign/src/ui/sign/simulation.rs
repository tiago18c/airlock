//! What the node says the transaction would do.

use super::*;
use crate::ui::{Glyph, Icon};

/// One quiet line about what the transaction would do, with the details a
/// click away.
#[component]
pub(in crate::ui) fn SimulationStrip(tx: std::rc::Rc<DecodedTx>, inspect: bool) -> Element {
    let session = use_context::<Session>();
    let state = (session.forms.sign.simulation)();
    let mut open = use_signal(|| false);
    let online = crate::state::use_network().is_some();
    let limit = tx.budget.compute_unit_limit;

    // An answer is a word and a mark; anything short of one keeps the dot.
    let (class, text, mark, reason) = match &state {
        SimState::Idle if online => ("sim", "Not simulated yet.".to_string(), None, None),
        SimState::Idle => (
            "sim",
            "Not simulated: this device is offline.".to_string(),
            None,
            None,
        ),
        SimState::Waiting(reason) => ("sim", reason.clone(), None, None),
        SimState::Running => ("sim running", "Simulating…".to_string(), None, None),
        SimState::Failed(err) => ("sim", format!("Could not simulate: {err}"), None, None),
        SimState::Done(simulation) => match &simulation.error {
            None => (
                "sim ok",
                "Simulation succeeds".to_string(),
                Some(Glyph::Check),
                None,
            ),
            Some(error) => (
                "sim fail",
                "Simulation failed".to_string(),
                Some(Glyph::X),
                Some(match error.instruction {
                    Some(index) => format!("instruction {}: {}", index + 1, error.message),
                    None => error.message.clone(),
                }),
            ),
        },
    };
    let done = match state {
        SimState::Done(simulation) => Some(simulation),
        _ => None,
    };
    let can_run = online && !matches!((session.forms.sign.simulation)(), SimState::Running);
    let again = tx.clone();

    rsx! {
        div { class: "{class}", role: "status",
            if let Some(glyph) = mark {
                Icon { glyph }
            } else {
                span { class: "status-dot" }
            }
            span { class: "sim-text",
                "{text}"
                if let Some(reason) = reason {
                    ": {reason}"
                }
            }
            if done.is_some() {
                button { class: "ghost sm",
                    onclick: move |_| open.set(!open()),
                    if open() { "Hide details" } else { "Details" }
                }
            }
            if can_run {
                button { class: "ghost sm",
                    onclick: move |_| start_simulation(session, again.as_ref().clone()),
                    "Run again"
                }
            }
        }
        if open() {
            if let Some(simulation) = done {
                SimulationDetails { simulation, limit, inspect }
            }
        }
    }
}

#[component]
pub(in crate::ui) fn SimulationDetails(
    simulation: Simulation,
    limit: Option<u32>,
    inspect: bool,
) -> Element {
    let session = use_context::<Session>();
    let logs = simulation.logs.join("\n");
    let calls: Vec<(usize, Vec<wallet_core::ExplainedIx>)> = simulation
        .inner
        .iter()
        .filter(|group| !group.calls.is_empty())
        .map(|group| {
            let instructions: Vec<wallet_core::Instruction> = group
                .calls
                .iter()
                .map(|call| call.instruction.clone())
                .collect();
            (
                group.instruction,
                explain_instructions(&*session.idls.read(), &instructions),
            )
        })
        .collect();
    rsx! {
        div { class: "sim-details",
            if let Some(units) = simulation.units_consumed {
                p {
                    "Used {units} compute units"
                    if let Some(limit) = limit {
                        " of the {limit} the transaction allows"
                    }
                    "."
                }
            }
            for (index, instructions) in calls {
                div { class: "stack", key: "{index}",
                    h4 { "Calls made by instruction {index + 1}" }
                    InstructionList { instructions, inspect }
                }
            }
            if let Some((program, data)) = simulation.return_data.clone() {
                div { class: "chip-row",
                    span { class: "muted", "returned by" }
                    Address { pubkey: program }
                    code { class: "mono", "{hex::encode(&data)}" }
                }
            }
            if !logs.is_empty() {
                h4 { "Program log" }
                pre { class: "logs", "{logs}" }
            }
        }
    }
}
