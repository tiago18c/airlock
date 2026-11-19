//! What a node says a transaction would do: one quiet line, with the details
//! a click away. Review and Sign shows it for the transaction on screen; the
//! Proposals modal and Review and Sign show it too for what a Squads
//! proposal would run, so both read as one kind of thing.

use crate::state::forms::SimState;
use crate::state::session::Session;
use crate::state::use_network;
use crate::ui::tx_view::{ReadInstructions, SimulatedDeltas};
use crate::ui::{Address, Glyph, Icon};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::group_thousands;
use wallet_core::simulate::Simulation;

/// The line for `state`. `limit`: the compute limit the transaction states,
/// which the details measure what it used against. `on_again`: a Run again
/// button. `changes`: list every balance that would change under the line,
/// for a transaction whose accounts no summary names beside it.
#[component]
pub fn SimulationStrip(
    state: Signal<SimState>,
    limit: Option<u32>,
    inspect: bool,
    #[props(default)] on_again: Option<EventHandler<()>>,
    #[props(default)] changes: bool,
) -> Element {
    let session = use_context::<Session>();
    let current = state();
    let mut open = use_signal(|| false);
    let online = use_network().is_some();

    // An answer is a word and a mark; anything short of one keeps the dot.
    let (class, text, mark, reason) = match &current {
        SimState::Idle if online => ("sim", "Not simulated yet.".to_string(), None, None),
        SimState::Idle => (
            "sim",
            "Not simulated: this app never connects.".to_string(),
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
                Some(wallet_core::word_simulation_error(
                    error,
                    &*session.idls.read(),
                )),
            ),
        },
    };
    let done = match current {
        SimState::Done(simulation) => Some(simulation),
        _ => None,
    };
    let changed: Vec<Pubkey> = match (&done, changes) {
        (Some(simulation), true) if simulation.error.is_none() => simulation
            .balance_changes
            .iter()
            .filter(|change| change.before != change.after)
            .map(|change| change.pubkey)
            .collect(),
        _ => Vec::new(),
    };
    let last = changed.len().saturating_sub(1);

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
            if let Some(again) = on_again {
                button { class: "ghost sm",
                    onclick: move |_| again.call(()),
                    "Run again"
                }
            }
        }
        if !changed.is_empty() {
            SimulatedDeltas { from: Some(state),
                p { class: "fee-line muted",
                    "Changes "
                    for (i, pubkey) in changed.into_iter().enumerate() {
                        // The comma closes its own item: the line's gap follows it.
                        span { key: "{pubkey}",
                            Address { pubkey, delta: true }
                            if i < last {
                                ","
                            }
                        }
                    }
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
pub fn SimulationDetails(simulation: Simulation, limit: Option<u32>, inspect: bool) -> Element {
    let logs = &simulation.logs;
    let calls: Vec<(usize, Vec<wallet_core::Instruction>)> = simulation
        .inner
        .iter()
        .filter(|group| !group.calls.is_empty())
        .map(|group| {
            (
                group.instruction,
                group
                    .calls
                    .iter()
                    .map(|call| call.instruction.clone())
                    .collect(),
            )
        })
        .collect();
    rsx! {
        div { class: "sim-details",
            if let Some(units) = simulation.units_consumed {
                p {
                    "Used {group_thousands(&units.to_string())} compute units"
                    if let Some(limit) = limit {
                        " of the {group_thousands(&limit.to_string())} the transaction allows"
                    }
                    "."
                }
            }
            for (index, instructions) in calls {
                div { class: "stack", key: "{index}",
                    h4 { "Calls made by instruction {index + 1}" }
                    ReadInstructions { instructions, inspect }
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
                ProgramLog { lines: simulation.logs.clone() }
            }
        }
    }
}

/// The program log, with each program the app knows by name written by
/// that name, its address in the line's tooltip. "Raw" shows the log as the
/// node returned it.
#[component]
fn ProgramLog(lines: Vec<String>) -> Element {
    let session = use_context::<Session>();
    let mut raw = use_signal(|| false);
    let named: Vec<(String, Option<String>)> = lines
        .iter()
        .map(
            |line| match name_programs(line, |pubkey| session.label_of(pubkey)) {
                Some((shown, address)) => (shown, Some(address)),
                None => (line.clone(), None),
            },
        )
        .collect();
    let raw_text = lines.join("\n");
    rsx! {
        div { class: "log-head",
            h4 { "Program log" }
            button {
                class: if raw() { "sm log-raw on" } else { "sm log-raw" },
                r#type: "button",
                aria_pressed: "{raw()}",
                title: "Show the log exactly as the node returned it, with every program by its address",
                onclick: move |_| raw.toggle(),
                "Raw"
            }
        }
        if raw() {
            pre { class: "logs", "{raw_text}" }
        } else {
            pre { class: "logs",
                for (i, (text, title)) in named.into_iter().enumerate() {
                    span { key: "{i}", class: "log-line", title, "{text}" }
                }
            }
        }
    }
}

/// `line` with the address of the program it is about written as that
/// program's name, and the address, or `None` when `name` does not know
/// it. A program's
/// address is the word after "Program " and after "Program return: ", as
/// the runtime writes them; what a program logs itself is left as it is.
fn name_programs(line: &str, name: impl Fn(&Pubkey) -> Option<String>) -> Option<(String, String)> {
    let (prefix, rest) = ["Program return: ", "Program "]
        .into_iter()
        .find_map(|prefix| Some((prefix, line.strip_prefix(prefix)?)))?;
    let (word, tail) = rest.split_at(rest.find(' ').unwrap_or(rest.len()));
    let label = name(&word.parse::<Pubkey>().ok()?)?;
    Some((format!("{prefix}{label}{tail}"), word.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_program_is_named_and_the_rest_of_the_log_is_left() {
        let system = Pubkey::default();
        let name = |pubkey: &Pubkey| (*pubkey == system).then(|| "System Program".to_string());
        assert_eq!(
            name_programs(&format!("Program {system} invoke [1]"), name),
            Some((
                "Program System Program invoke [1]".to_string(),
                system.to_string()
            ))
        );
        assert_eq!(
            name_programs(&format!("Program return: {system} AQID"), name).map(|(text, _)| text),
            Some("Program return: System Program AQID".to_string())
        );
        // A program the app does not know, and what a program logs itself.
        let other = Pubkey::new_from_array([7; 32]);
        assert_eq!(
            name_programs(&format!("Program {other} success"), name),
            None
        );
        assert_eq!(name_programs(&format!("Program log: {system}"), name), None);
    }
}
