//! A change to a Squads multisig itself, in words: one card per action, in
//! order. The cards carry the colour of a warning, since a change to who
//! controls the multisig is the most serious thing a member approves; they
//! stand in for separate warnings, which would say the same thing twice. A
//! change the program would refuse says so after them. The Proposals modal
//! shows it in a proposal's row, and Review and Sign under "Stored on
//! chain", so a change reads the same wherever it is voted on.
//!
//! Each address is named as everywhere else, through [`Address`], and the
//! member this device acts as is marked "you", as the votes of a proposal
//! mark it.

use super::Address;
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::squads::{
    change_lines, change_outcome, ConfigTransaction, Piece, Said, SquadState,
};

/// One sentence, with its addresses named.
#[component]
pub fn SaidLine(
    said: Said,
    /// The member this device acts as, marked "you".
    #[props(default)]
    acting: Option<Pubkey>,
    /// The class of the paragraph: "warn", "hint", or none.
    #[props(default)]
    class: &'static str,
) -> Element {
    rsx! {
        p { class: "chip-row change-line {class}",
            for (at, piece) in said.0.into_iter().enumerate() {
                match piece {
                    Piece::Text(text) => rsx! { span { key: "{at}", "{text}" } },
                    Piece::Key(key) => rsx! {
                        span { key: "{at}", class: "proposal-voter",
                            Address { pubkey: key }
                            if acting == Some(key) {
                                span { class: "chip", "you" }
                            }
                        }
                    },
                }
            }
        }
    }
}

/// What `change` does to the multisig `state` (when it is known: a device
/// with no network may only have the change), voted on by `acting`, at
/// time `now` in Unix seconds.
///
/// `settled`: it can no longer run on the multisig as it is (executed,
/// rejected, cancelled, or stale), so only its actions are listed, without
/// what they would change from or leave.
#[component]
pub fn MultisigChange(
    change: ConfigTransaction,
    state: Option<SquadState>,
    #[props(default)] acting: Option<Pubkey>,
    now: i64,
    #[props(default)] settled: bool,
) -> Element {
    let state = state.filter(|_| !settled);
    let lines = change_lines(state.as_ref(), &change.actions);
    let refusal = state
        .as_ref()
        .and_then(|state| change_outcome(change.program, state, &change.actions, now).err())
        .map(|refusal| refusal.said());
    let count = lines.len();
    rsx! {
        div { class: "multisig-change stack",
            p { class: "muted",
                if count == 1 {
                    "A change to the multisig itself, with one action:"
                } else {
                    "A change to the multisig itself, with {count} actions, in this order:"
                }
            }
            ol { class: "change-actions",
                for (at, said) in lines.into_iter().enumerate() {
                    li { key: "{at}", class: "change-card",
                        if count > 1 {
                            span { class: "change-number", "{at + 1}" }
                        }
                        SaidLine { said, acting }
                    }
                }
            }
            if let Some(said) = refusal {
                SaidLine { said, acting, class: "error" }
            }
            if let Some(why) = change.not_executed_here().filter(|_| !settled) {
                p { class: "hint", "{why}" }
            }
        }
    }
}
