//! Proposing a change to a multisig itself, from its Settings: a member
//! added or removed, the threshold, and on v4 the rent collector. Unlike
//! the rest of Settings, which is what this device keeps, a change is
//! proposed, voted and executed on chain, so it has its own section and its
//! own button, and acts through Review and Sign like every transaction.
//!
//! The form is fields and one button. What the multisig would be after it
//! shows in one line when that differs from now, and what cannot be
//! proposed in one sentence in place of an enabled button. What a voter
//! should weigh is shown where the vote is cast, in the proposal's row and
//! on Review and Sign, not here. Online only: the multisig is read again as
//! the section opens, and again when Propose is pressed.

use super::*;
use crate::ui::{AddressPicker, SaidLine};
use wallet_core::proposals::{change_proposal, vet_change, CannotPropose, ChangeDraft};
use wallet_core::squads::{
    SquadsMember, PERMISSION_ALL, PERMISSION_EXECUTE, PERMISSION_INITIATE, PERMISSION_VOTE,
};

/// The section of a multisig's Settings that proposes a change to it.
/// `on_done` runs once the transaction is open on Review and Sign.
#[component]
pub(in crate::ui) fn ChangeSection(id: WalletId, on_done: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let online = crate::state::use_network().is_some();
    // Read the multisig again as the section opens, so the form starts from
    // the multisig as it is.
    use_hook(move || {
        if online {
            spawn(async move {
                crate::actions::squads::refresh_quietly(session, Some(vec![id])).await;
            });
        }
    });
    let mut add = use_signal(|| None::<Pubkey>);
    let mut may = use_signal(|| PERMISSION_ALL);
    let mut remove = use_signal(|| None::<Pubkey>);
    let mut threshold = use_signal(|| None::<String>);
    let mut collector = use_signal(|| None::<Option<Pubkey>>);
    let add_reset = use_signal(|| 0u64);
    let collector_reset = use_signal(|| 0u64);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let wallets = (session.wallets)().summaries.clone();
    let kind = wallets
        .iter()
        .find(|wallet| wallet.id == id)
        .and_then(|wallet| wallet.contract.clone());
    let (Some(squad), Some(state), true) = (
        kind.as_ref().and_then(WalletKind::squad),
        kind.as_ref().and_then(WalletKind::squad_state),
        online,
    ) else {
        return rsx! {};
    };
    // Who proposes, as the record knows them: one that cannot gets one
    // sentence and no form, as a controlled multisig does.
    let (chosen, row_nonce) = session.chosen_members(id, &state);
    if let Err(why) = change_proposal(&state, &chosen, row_nonce) {
        return rsx! {
            div { class: "stack change-section",
                h4 { class: "eyebrow", "Propose a change" }
                SaidLine { said: why.said(), class: "muted" }
            }
        };
    }

    let draft = ChangeDraft {
        add: add().map(|key| SquadsMember {
            key,
            permissions: may(),
        }),
        remove: remove(),
        threshold: threshold().map(|typed| typed.trim().parse().unwrap_or(0)),
        rent_collector: collector(),
    };
    let actions = draft.actions(&state);
    let vetted = vet_change(squad.program, &state, &actions, crate::actions::unix_now());
    let refusal = match &vetted {
        Err(CannotPropose::NoActions) | Ok(_) => None,
        Err(why) => Some(why.said()),
    };
    let ready = vetted.is_ok() && !busy();
    let shown_threshold = threshold().unwrap_or_else(|| state.threshold.to_string());
    let shown_collector = collector().unwrap_or(state.rent_collector);
    let members: Vec<Pubkey> = state.members.iter().map(|member| member.key).collect();
    let v4 = squad.program == SquadsProgram::V4;

    let propose = move |_| {
        error.set(None);
        let draft = draft.clone();
        session.when_unlocked(CHANGE_WALLET, move || {
            busy.set(true);
            spawn(async move {
                let made = crate::actions::squads::propose_change(session, id, draft).await;
                busy.set(false);
                match made {
                    Ok(()) => on_done.call(()),
                    Err(why) => error.set(Some(why)),
                }
            });
        });
    };
    let permission = move |bit: u8, word: &'static str| {
        rsx! {
            label { class: "check", key: "{bit}",
                input {
                    r#type: "checkbox",
                    checked: may() & bit != 0,
                    onchange: move |evt| {
                        let on = evt.checked();
                        may.with_mut(|may| if on { *may |= bit } else { *may &= !bit });
                    },
                }
                "{word}"
            }
        }
    };

    rsx! {
        div { class: "stack change-section",
            h4 { class: "eyebrow", "Propose a change" }
            div { class: "settings-grid",
                AddressPicker {
                    label: "Add member".to_string(),
                    selected: add(),
                    reset: add_reset,
                    on_change: move |picked| add.set(picked),
                }
                if add().is_some() {
                    div { class: "setting-row",
                        span { class: "setting-label", "Permissions" }
                        div { class: "setting-value",
                            {permission(PERMISSION_INITIATE, "propose")}
                            {permission(PERMISSION_VOTE, "vote")}
                            {permission(PERMISSION_EXECUTE, "execute")}
                        }
                    }
                }
                AddressSelect {
                    label: "Remove member".to_string(),
                    options: members,
                    selected: remove(),
                    none_label: "No one".to_string(),
                    on_change: move |picked| remove.set(picked),
                }
                div { class: "setting-row",
                    span { class: "setting-label", "Threshold" }
                    div { class: "setting-value",
                        input {
                            class: "change-threshold",
                            r#type: "text",
                            inputmode: "numeric",
                            aria_label: "Threshold",
                            value: "{shown_threshold}",
                            oninput: move |evt| threshold.set(Some(evt.value())),
                        }
                    }
                }
                if v4 {
                    AddressPicker {
                        label: "Rent collector".to_string(),
                        selected: shown_collector,
                        reset: collector_reset,
                        on_change: move |picked| collector.set(Some(picked)),
                    }
                }
            }
            if let Some(said) = refusal {
                SaidLine { said, class: "warn" }
            }
            if let Some(why) = error() {
                p { class: "error", role: "alert", "{why}" }
            }
            div { class: "btn-row change-propose",
                button {
                    r#type: "button",
                    disabled: !ready,
                    onclick: propose,
                    if busy() { "Preparing…" } else { "Propose" }
                }
            }
        }
    }
}
