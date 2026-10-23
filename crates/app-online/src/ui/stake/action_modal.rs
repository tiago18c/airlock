//! The modal that collects what one stake operation needs.

use super::*;

#[component]
pub(in crate::ui) fn ActionModal(action: Action, on_close: EventHandler<()>) -> Element {
    let session = use_context::<Session>();
    let wallets = owners(session);
    let kind = action.kind;
    let target = action.target.clone();
    let snapshot = (session.forms.stake.snapshot)();
    let epoch = snapshot
        .as_ref()
        .map(|found| found.epoch.epoch)
        .unwrap_or(0);
    let rent = snapshot
        .as_ref()
        .map(|found| found.rent_reserve)
        .unwrap_or(STAKE_RENT_LAMPORTS);
    let min_delegation = snapshot
        .as_ref()
        .map(|found| found.min_delegation)
        .unwrap_or(0);
    // Whether an account holding this much could be delegated.
    let delegable = {
        let snapshot = snapshot.clone();
        move |lamports: u64| {
            snapshot
                .as_ref()
                .is_none_or(|found| found.can_delegate(lamports))
        }
    };
    let too_small = format!(
        "A delegation needs at least {} on top of the rent reserve of {}.",
        format_sol(min_delegation.max(1)),
        format_sol(rent)
    );

    let mut wallet = use_signal({
        let wallets = wallets.clone();
        move || {
            session
                .selected_wallet()
                .filter(|id| wallets.iter().any(|wallet| wallet.id == *id))
                .or_else(|| wallets.first().map(|wallet| wallet.id))
        }
    });
    let mut amount = use_signal(String::new);
    // The validator, the destination or the new authority, by `kind`.
    let mut address = use_signal({
        let target = target.clone();
        move || match (kind, &target) {
            (Kind::Withdraw, Some(target)) => Some(target.owner),
            _ => None,
        }
    });
    let picker_reset = use_signal(|| 0u64);
    let mut role = use_signal(|| StakeRole::Staker);
    let mut merge_source = use_signal(|| None::<Pubkey>);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let withdrawable = target
        .as_ref()
        .map(|target| target.stake.withdrawable(epoch))
        .unwrap_or(0);
    let is_withdrawer = target.as_ref().is_some_and(|target| {
        target
            .stake
            .account
            .has_role(&target.owner, StakeRole::Withdrawer)
    });
    // Accounts the stake program would merge into this one.
    let candidates: Vec<ListedStake> = match (&target, &snapshot) {
        (Some(target), Some(found)) if kind == Kind::Merge => found
            .stakes
            .iter()
            .filter(|other| {
                target.stake.account.can_absorb(
                    &target.stake.status,
                    &other.account,
                    &other.status,
                    epoch,
                )
            })
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    let first_candidate = candidates.first().map(|stake| stake.account.pubkey);

    let submit = {
        let target = target.clone();
        let too_small = too_small.clone();
        move |_| {
            error.set(None);
            let typed = amount().trim().to_string();
            let request = (|| -> Result<(WalletId, StakeRequest), String> {
                let target = target.clone();
                let on_target = || {
                    target
                        .clone()
                        .ok_or("No stake account was chosen.".to_string())
                };
                Ok(match kind {
                    Kind::New => {
                        let lamports = sol_to_lamports(&typed).map_err(|err| err.to_string())?;
                        if address().is_some() && !delegable(lamports) {
                            return Err(too_small.clone());
                        }
                        (
                            wallet().ok_or("Pick the wallet that funds the stake.")?,
                            StakeRequest::New {
                                lamports,
                                vote: address(),
                            },
                        )
                    }
                    Kind::Delegate => {
                        let target = on_target()?;
                        if !delegable(target.stake.account.lamports) {
                            return Err(too_small.clone());
                        }
                        (
                            target.wallet,
                            StakeRequest::Delegate {
                                stake: target.stake.account.pubkey,
                                vote: address().ok_or("Enter the validator's vote account.")?,
                            },
                        )
                    }
                    Kind::Deactivate => {
                        let target = on_target()?;
                        (
                            target.wallet,
                            StakeRequest::Deactivate {
                                stake: target.stake.account.pubkey,
                            },
                        )
                    }
                    Kind::Withdraw => {
                        let target = on_target()?;
                        let requested = if typed.is_empty() {
                            None
                        } else {
                            Some(sol_to_lamports(&typed).map_err(|err| err.to_string())?)
                        };
                        let lamports = target
                            .stake
                            .account
                            .withdraw_amount(epoch, &target.stake.status, requested)
                            .map_err(|err| err.to_string())?;
                        (
                            target.wallet,
                            StakeRequest::Withdraw {
                                stake: target.stake.account.pubkey,
                                lamports,
                                to: address().ok_or("Enter the address that receives the SOL.")?,
                            },
                        )
                    }
                    Kind::Split => {
                        let target = on_target()?;
                        let lamports = sol_to_lamports(&typed).map_err(|err| err.to_string())?;
                        if lamports == 0 || lamports >= target.stake.account.lamports {
                            return Err("Enter less than the account holds. To move all of it, change its authorities instead.".into());
                        }
                        (
                            target.wallet,
                            StakeRequest::Split {
                                account: target.stake.account,
                                lamports,
                            },
                        )
                    }
                    Kind::Merge => {
                        let target = on_target()?;
                        (
                            target.wallet,
                            StakeRequest::Merge {
                                stake: target.stake.account.pubkey,
                                source: merge_source()
                                    .or(first_candidate)
                                    .ok_or("No other stake account can be merged into this one.")?,
                            },
                        )
                    }
                    Kind::Authorize => {
                        let target = on_target()?;
                        let new_authority =
                            address().ok_or("Enter the new authority's address.")?;
                        // Only the withdrawer may hand over withdrawals.
                        let role = if is_withdrawer {
                            role()
                        } else {
                            StakeRole::Staker
                        };
                        (
                            target.wallet,
                            StakeRequest::Authorize {
                                stake: target.stake.account.pubkey,
                                new_authority,
                                role,
                            },
                        )
                    }
                })
            })();
            let (wallet, request) = match request {
                Ok(request) => request,
                Err(err) => {
                    error.set(Some(err));
                    return;
                }
            };
            busy.set(true);
            spawn(async move {
                match crate::actions::stake::prepare(session, wallet, request).await {
                    Ok(()) => on_close.call(()),
                    Err(err) => error.set(Some(err)),
                }
                busy.set(false);
            });
        }
    };

    let subject = target
        .as_ref()
        .map(|target| short_pubkey(&target.stake.account.pubkey))
        .unwrap_or_default();
    let title = match kind {
        Kind::New => "New stake".to_string(),
        Kind::Delegate => format!("Delegate {subject}"),
        Kind::Deactivate => format!("Deactivate {subject}"),
        Kind::Withdraw => format!("Withdraw from {subject}"),
        Kind::Split => format!("Split {subject}"),
        Kind::Merge => format!("Merge into {subject}"),
        Kind::Authorize => format!("Authorities of {subject}"),
    };
    let lede = match kind {
        Kind::New => "Creates a stake account funded by the wallet, with the wallet as its stake and withdraw authority. The account's address is derived from the wallet's, so there is no new key to keep.",
        Kind::Delegate => "The whole account is delegated to one validator. It starts earning at the start of the next epoch.",
        Kind::Deactivate => "The stake keeps earning until this epoch ends. After that everything in the account can be withdrawn or delegated again.",
        Kind::Withdraw => "Withdrawing everything closes the account. Otherwise the account keeps at least its rent reserve.",
        Kind::Split => "Moves part of this account into a new stake account with the same authorities, validator and state.",
        Kind::Merge => "The chosen account is closed and what it holds is added to this one. Both need the same authorities and compatible states.",
        Kind::Authorize => "Hands one authority to another address. It takes effect at once.",
    };

    rsx! {
        div { class: "modal-backdrop", onclick: move |_| on_close.call(()),
            div { class: "modal", onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "{title}" }
                    p { class: "lede", "{lede}" }

                    if kind == Kind::New {
                        WalletPicker {
                            label: "Wallet",
                            options: wallets.clone(),
                            selected: wallet(),
                            none_label: "Choose a wallet",
                            on_change: move |choice| wallet.set(choice),
                        }
                    }
                    if matches!(kind, Kind::New | Kind::Withdraw | Kind::Split) {
                        label { "Amount (SOL)"
                            input {
                                r#type: "text",
                                inputmode: "decimal",
                                placeholder: if kind == Kind::Withdraw { "Everything available" } else { "1.5" },
                                value: "{amount}",
                                oninput: move |evt| amount.set(evt.value()),
                            }
                        }
                    }
                    if kind == Kind::New {
                        p { class: "hint",
                            "Of this, {format_sol(rent)} is the account's rent reserve. It is not staked, and comes back when the account is closed."
                        }
                    }
                    if kind == Kind::Withdraw {
                        p { class: "hint", "Up to {format_sol(withdrawable)} can be withdrawn now. Leave the amount blank to take all of it." }
                    }
                    if kind == Kind::Split {
                        p { class: "hint",
                            "The new account's rent reserve of {format_sol(rent)} is paid by the wallet, on top of the amount."
                        }
                    }
                    if kind == Kind::Merge {
                        label { "Account to merge in"
                            select {
                                onchange: move |evt| merge_source.set(evt.value().parse().ok()),
                                for stake in candidates.clone() {
                                    option {
                                        key: "{stake.account.pubkey}",
                                        value: "{stake.account.pubkey}",
                                        selected: merge_source().or(first_candidate) == Some(stake.account.pubkey),
                                        "{short_pubkey(&stake.account.pubkey)} · {format_sol(stake.account.lamports)}"
                                    }
                                }
                            }
                        }
                    }
                    if kind == Kind::Authorize {
                        label { "Authority"
                            select {
                                onchange: move |evt| {
                                    role.set(if evt.value() == "withdrawer" {
                                        StakeRole::Withdrawer
                                    } else {
                                        StakeRole::Staker
                                    });
                                },
                                option { value: "staker", selected: role() == StakeRole::Staker,
                                    "Stake authority: delegates, deactivates, splits and merges"
                                }
                                if is_withdrawer {
                                    option { value: "withdrawer", selected: role() == StakeRole::Withdrawer,
                                        "Withdraw authority: withdraws, and can replace both"
                                    }
                                }
                            }
                        }
                    }
                    if matches!(kind, Kind::New | Kind::Delegate | Kind::Withdraw | Kind::Authorize) {
                        AddressPicker {
                            label: match kind {
                                Kind::New => "Validator vote account (optional)",
                                Kind::Delegate => "Validator vote account",
                                Kind::Withdraw => "Send to",
                                _ => "New authority",
                            }.to_string(),
                            selected: address(),
                            reset: picker_reset,
                            on_change: move |picked| address.set(picked),
                        }
                    }
                    if kind == Kind::New {
                        p { class: "hint",
                            "Leave the validator blank to create the account now and delegate it later."
                            if min_delegation > 1 {
                                " A delegation needs at least {format_sol(min_delegation)} of stake."
                            }
                        }
                    }
                    if kind == Kind::Authorize {
                        p { class: "warn",
                            "An address that is not a wallet on this device takes this account out of your hands for that role. Nothing here can undo it."
                        }
                    }

                    if let Some(err) = error() {
                        p { class: "error", "{err}" }
                    }
                    div { class: "btn-row",
                        button { class: "primary", disabled: busy(), onclick: submit,
                            if busy() { "Preparing…" } else { "Review in Sign" }
                        }
                        button { class: "ghost", onclick: move |_| on_close.call(()), "Cancel" }
                    }
                }
            }
        }
    }
}
