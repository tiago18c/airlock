//! Migrate wallet: moving what a wallet holds to another address, to a
//! wallet of this vault (another kind, or a new key) or to an address pasted
//! in. Wallets opens it. Pick the wallet and where it goes, tick what moves
//! by group and item, then sign the steps one after another; the last sends
//! the SOL when it was ticked.

use app_ui_kit::actions::migrate;
use app_ui_kit::{
    format_sol, format_token_amount, AddressPicker, AddressRow, Session, WalletPicker,
};
use dioxus::prelude::*;
use solana_pubkey::Pubkey;
use wallet_core::migrate::{NonceBlocked, NonceOutcome, StakeBlocked, StakeChoice, StakeOutcome};
use wallet_core::spl::is_native_mint;
use wallet_core::token_plan::{Blocked, Held, Outcome};
use wallet_core::{short_pubkey, TokenFlag, WalletId};
use wallet_flows::migrate::{Choices, DestinationCheck, MigrationPlan, Snapshot, TokenMove};

/// What a burn asks to be typed before the move starts.
const BURN_WORD: &str = "burn";

/// Mounts the Migrate wallet modal while Wallets has it open.
#[component]
pub fn MigrateHost() -> Element {
    let session = use_context::<Session>();
    match (session.forms.migrate.open)() {
        Some(from) => rsx! { MigrateModal { from } },
        None => rsx! {},
    }
}

#[derive(Clone, PartialEq)]
enum Check {
    Idle,
    Checking,
    Ok(DestinationCheck),
    Refused(String),
}

#[derive(Clone, PartialEq)]
enum Holdings {
    Idle,
    Reading,
    Read(Box<Snapshot>),
    Failed(String),
}

#[component]
fn MigrateModal(from: Option<WalletId>) -> Element {
    let session = use_context::<Session>();
    let form = session.forms.migrate;
    let kept = form.run.peek().clone();
    let mut source = use_signal(|| kept.as_ref().map(|run| run.from).or(from));
    let finished_to = form
        .finished
        .peek()
        .filter(|(wallet, _)| Some(*wallet) == from)
        .map(|(_, to)| to);
    let mut to = use_signal(|| kept.as_ref().map(|run| run.to).or(finished_to));
    let reset = use_signal(|| 0u64);
    let mut check = use_signal(|| Check::Idle);
    let mut holdings = use_signal(|| Holdings::Idle);
    let mut choices = use_signal(Choices::default);
    let mut typed_burn = use_signal(String::new);
    let mut typed_address = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut notice = form.notice;
    // Answers to an older pair are dropped.
    let mut asked = use_signal(|| 0u64);

    let mut reread = move || {
        let (Some(from), Some(dest)) = (*source.peek(), *to.peek()) else {
            check.set(Check::Idle);
            holdings.set(Holdings::Idle);
            return;
        };
        let generation = *asked.peek() + 1;
        asked.set(generation);
        check.set(Check::Checking);
        holdings.set(Holdings::Idle);
        spawn(async move {
            let checked = migrate::check(session, from, dest).await;
            if *asked.peek() != generation {
                return;
            }
            match checked {
                Ok(found) => check.set(Check::Ok(found)),
                Err(why) => {
                    check.set(Check::Refused(why));
                    return;
                }
            }
            holdings.set(Holdings::Reading);
            let read = migrate::read(session, from, dest).await;
            if *asked.peek() != generation {
                return;
            }
            match read {
                Ok(snapshot) => {
                    // A move under way keeps what was chosen for it.
                    let run = form
                        .run
                        .peek()
                        .clone()
                        .filter(|run| run.from == from && run.to == dest);
                    choices.set(match run {
                        Some(run) => run.choices,
                        None => migrate::default_choices(session, from, &snapshot),
                    });
                    holdings.set(Holdings::Read(Box::new(snapshot)));
                }
                Err(why) => holdings.set(Holdings::Failed(why)),
            }
        });
    };
    use_effect(move || {
        let _ = (source(), to());
        reread();
    });

    let wallets: Vec<_> = session
        .wallet_summaries()
        .into_iter()
        .filter(|wallet| wallet.address.is_some())
        .collect();
    let source_summary =
        source().and_then(|id| wallets.iter().find(|wallet| wallet.id == id).cloned());
    let source_label = source_summary
        .as_ref()
        .map(|wallet| wallet.label.clone())
        .unwrap_or_default();
    let dest = to();
    let dest_wallet = dest.and_then(|address| {
        wallets
            .iter()
            .find(|wallet| wallet.address == Some(address))
            .cloned()
    });
    let pasted = dest.is_some() && dest_wallet.is_none();
    let unused = matches!(check(), Check::Ok(DestinationCheck { unused: true })) && pasted;
    let address_confirmed = !unused
        || dest.is_some_and(|dest| {
            let text = dest.to_string();
            let tail = &text[text.len() - 4..];
            typed_address().trim() == tail
        });

    let snapshot = match holdings() {
        Holdings::Read(snapshot) => Some(*snapshot),
        _ => None,
    };
    let plan: Option<Result<MigrationPlan, String>> = match (source(), &snapshot) {
        (Some(from), Some(snapshot)) => Some(migrate::plan(session, from, snapshot, &choices())),
        _ => None,
    };
    let plan_ok = plan.as_ref().and_then(|plan| plan.as_ref().ok()).cloned();
    let steps = plan_ok.as_ref().map_or(0, MigrationPlan::steps);
    let burns = plan_ok.as_ref().map_or(0, |plan| {
        plan.tokens
            .accounts
            .iter()
            .filter(|planned| matches!(planned.outcome, Outcome::Burned { .. }))
            .count()
    });
    let burn_confirmed = burns == 0 || typed_burn().trim().eq_ignore_ascii_case(BURN_WORD);
    let run = form
        .run
        .read()
        .clone()
        .filter(|run| Some(run.from) == source() && Some(run.to) == dest);
    let waiting = run.as_ref().is_some_and(|run| run.waiting);
    let can_start = steps > 0
        && burn_confirmed
        && address_confirmed
        && !busy()
        && matches!(check(), Check::Ok(_));

    let close = move || session.close_migrate();
    let begin = move |_| {
        let (Some(from), Some(dest)) = (source(), to()) else {
            return;
        };
        error.set(None);
        busy.set(true);
        let picks = choices();
        let resuming = form
            .run
            .peek()
            .as_ref()
            .is_some_and(|run| run.from == from && run.to == dest);
        spawn(async move {
            let result = if resuming {
                migrate::resume(session).await
            } else {
                migrate::start(session, from, dest, picks, 0)
                    .await
                    .map(|started| match started {
                        migrate::Started::OnSign => None,
                        migrate::Started::Done => Some("Nothing you chose is left to move.".into()),
                    })
            };
            match result {
                Ok(None) => {}
                Ok(Some(why)) => {
                    notice.set(Some(why));
                    reread();
                }
                Err(why) => error.set(Some(why)),
            }
            busy.set(false);
        });
    };

    let start_label = match (&run, waiting) {
        (Some(_), true) => "Check again".to_string(),
        (Some(run), false) => format!("Go on: step {}", run.done + 1),
        (None, _) if burns > 0 => "Burn and migrate".to_string(),
        (None, _) => "Migrate".to_string(),
    };
    let references = source()
        .map(|from| migrate::references(&session, from))
        .unwrap_or_default();
    let finished = steps == 0 && snapshot.is_some();

    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| close(),
            div { class: "modal wide migrate",
                role: "dialog",
                aria_modal: "true",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "stack",
                    h3 { "Migrate wallet" }
                    p { class: "lede",
                        "Move what a wallet holds to another address: any of its tokens, stake accounts, the nonce accounts it controls, and its SOL. Use it to rotate a key, or to move from a plain key to a programmatic signer, a Vector account or a multisig."
                    }
                    if let Some(text) = notice() {
                        p { class: "note", role: "status", "{text}" }
                    }
                    WalletPicker {
                        label: "From".to_string(),
                        options: wallets.clone(),
                        selected: source(),
                        none_label: "Choose the wallet to move".to_string(),
                        on_change: move |id: Option<WalletId>| {
                            source.set(id);
                            notice.set(None);
                            error.set(None);
                        },
                    }
                    AddressPicker {
                        label: "To".to_string(),
                        selected: to(),
                        reset,
                        on_change: move |pk| {
                            to.set(pk);
                            typed_address.set(String::new());
                            notice.set(None);
                            error.set(None);
                        },
                    }
                    match check() {
                        Check::Idle => rsx! {
                            p { class: "hint", "Pick a wallet of this vault, or paste the address it goes to." }
                        },
                        Check::Checking => rsx! { p { class: "hint", role: "status", "Checking the address…" } },
                        Check::Refused(why) => rsx! { p { class: "error", "{why}" } },
                        Check::Ok(_) => rsx! {
                            if let Some(wallet) = &dest_wallet {
                                p { class: "hint", "It goes to {wallet.label}, a wallet of this vault." }
                            }
                        },
                    }
                    if unused {
                        div { class: "callout warn-callout",
                            p {
                                "Nothing shows that anyone uses this address: it holds no SOL and the node knows no transaction that names it. Whatever moves there is gone if nobody holds its key. Check it against the wallet that will hold it."
                            }
                            label { "Type its last 4 characters to confirm"
                                input {
                                    r#type: "text",
                                    autocomplete: "off",
                                    value: "{typed_address}",
                                    oninput: move |evt| typed_address.set(evt.value()),
                                }
                            }
                        }
                    }
                    match holdings() {
                        Holdings::Idle => rsx! {},
                        Holdings::Reading => rsx! {
                            p { class: "hint", role: "status", "Reading what {source_label} holds…" }
                        },
                        Holdings::Failed(why) => rsx! {
                            p { class: "error", "{why}" }
                            div { class: "btn-row",
                                button { onclick: move |_| reread(), "Try again" }
                            }
                        },
                        Holdings::Read(_) => rsx! {},
                    }
                    if let (Some(snapshot), Some(plan)) = (&snapshot, &plan_ok) {
                        Review {
                            snapshot: snapshot.clone(),
                            plan: plan.clone(),
                            choices: choices(),
                            on_change: move |next: Choices| choices.set(next),
                        }
                    }
                    if let Some(Err(why)) = &plan {
                        p { class: "error", "{why}" }
                    }
                    if steps > 0 {
                        p { class: "hint",
                            if steps == 1 {
                                "It takes one transaction."
                            } else {
                                "It takes {steps} transactions, signed one after another. Each is built once the one before it has landed."
                            }
                        }
                    }
                    if finished && run.is_none() {
                        Finished {
                            from: source(),
                            to: dest,
                            to_wallet: dest_wallet.as_ref().map(|wallet| wallet.id),
                            references: references.clone(),
                        }
                    }
                    if burns > 0 {
                        div { class: "callout danger-callout",
                            p {
                                if burns == 1 {
                                    "This burns the tokens of one account instead of moving them. Burned tokens are gone for good."
                                } else {
                                    "This burns the tokens of {burns} accounts instead of moving them. Burned tokens are gone for good."
                                }
                            }
                            label { "Type {BURN_WORD} to confirm"
                                input {
                                    r#type: "text",
                                    autocomplete: "off",
                                    value: "{typed_burn}",
                                    oninput: move |evt| typed_burn.set(evt.value()),
                                }
                            }
                        }
                    }
                    if let Some(why) = error() {
                        p { class: "error", "{why}" }
                    }
                    div { class: "btn-row",
                        button { class: "ghost",
                            onclick: move |_| close(),
                            if finished { "Close" } else { "Cancel" }
                        }
                        if !finished || run.is_some() {
                            button { class: if burns > 0 { "danger" } else { "primary" },
                                disabled: !(can_start || (waiting && !busy())),
                                onclick: begin,
                                if busy() { "Preparing…" } else { "{start_label}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// What moves, by group. A group's box ticks or unticks all of it; an
/// item's box ticks that item alone, and with it its group.
#[component]
fn Review(
    snapshot: Snapshot,
    plan: MigrationPlan,
    choices: Choices,
    on_change: EventHandler<Choices>,
) -> Element {
    let session = use_context::<Session>();
    let to = snapshot.to;
    // What an item is when it is ticked: a token is moved, unless the user
    // flagged it red; stake accounts hand both authorities over.
    let token_pick = move |held: &Held| {
        let flagged = session.flag_of(&held.address) == Some(TokenFlag::Red);
        if flagged && held.account.amount > 0 && !is_native_mint(&held.account.mint) {
            TokenMove::Burn
        } else {
            TokenMove::Move
        }
    };
    let tokens_on = choices
        .tokens
        .values()
        .filter(|pick| **pick != TokenMove::Leave)
        .count();
    let stakes_on = choices
        .stakes
        .values()
        .filter(|pick| pick.staker || pick.withdrawer)
        .count();
    let closable: Vec<Pubkey> = plan
        .nonces
        .iter()
        .filter(|(_, outcome)| {
            !matches!(outcome, NonceOutcome::Left(why) if *why != NonceBlocked::Chosen)
        })
        .map(|(address, _)| *address)
        .collect();
    let nonces_on = choices.nonces.len();

    let all_tokens = {
        let (base, tokens) = (choices.clone(), snapshot.tokens.clone());
        move |on: bool| {
            let mut next = base.clone();
            for held in &tokens {
                let pick = if on {
                    token_pick(held)
                } else {
                    TokenMove::Leave
                };
                next.tokens.insert(held.address, pick);
            }
            on_change.call(next);
        }
    };
    let all_stakes = {
        let (base, stakes) = (choices.clone(), snapshot.stakes.clone());
        move |on: bool| {
            let mut next = base.clone();
            for stake in &stakes {
                let pick = if on {
                    StakeChoice::BOTH
                } else {
                    StakeChoice::default()
                };
                next.stakes.insert(stake.pubkey, pick);
            }
            on_change.call(next);
        }
    };
    let all_nonces = {
        let (base, closable) = (choices.clone(), closable.clone());
        move |on: bool| {
            let mut next = base.clone();
            next.nonces = if on {
                closable.iter().copied().collect()
            } else {
                Default::default()
            };
            on_change.call(next);
        }
    };
    let sol_toggle = {
        let base = choices.clone();
        move |on: bool| {
            let mut next = base.clone();
            next.sol = on;
            on_change.call(next);
        }
    };

    rsx! {
        if !snapshot.tokens.is_empty() {
            Group {
                title: "Token accounts",
                on: tokens_on,
                of: snapshot.tokens.len(),
                hint: "Tokens go to the new address and each account is closed, its rent back to the wallet. Empty accounts are only closed.",
                on_toggle: all_tokens,
                ul { class: "book-list migrate-tokens",
                    for held in snapshot.tokens.iter().cloned() {
                        {
                            let address = held.address;
                            let native = is_native_mint(&held.account.mint);
                            let decimals = held.mint.as_ref().map_or(0, |mint| mint.decimals);
                            let amount = if native {
                                format_sol(held.account.amount)
                            } else {
                                format_token_amount(held.account.amount, decimals)
                            };
                            let outcome = plan
                                .tokens
                                .accounts
                                .iter()
                                .find(|planned| planned.address == address)
                                .map(|planned| planned.outcome.clone());
                            let current = choices.tokens.get(&address).copied().unwrap_or(TokenMove::Leave);
                            let ticked = current != TokenMove::Leave;
                            // Only an account with tokens in it can go either way.
                            let either = held.account.amount > 0 && !native;
                            let blocked = match &outcome {
                                Some(Outcome::Left(why)) if *why != Blocked::Chosen => Some(why.reason()),
                                _ => None,
                            };
                            let what = if native {
                                "Unwrap"
                            } else if held.account.amount == 0 {
                                "Close"
                            } else {
                                ""
                            };
                            let pick = token_pick(&held);
                            let (base, base2, base3) = (choices.clone(), choices.clone(), choices.clone());
                            rsx! {
                                li { class: "book-row pick-row", key: "{address}",
                                    input {
                                        r#type: "checkbox",
                                        checked: ticked,
                                        title: "Include this account",
                                        aria_label: "Include the account {address}",
                                        onchange: move |evt| {
                                            let mut next = base3.clone();
                                            next.tokens.insert(address, if evt.checked() { pick } else { TokenMove::Leave });
                                            on_change.call(next);
                                        },
                                    }
                                    AddressRow { pubkey: held.account.mint,
                                        span { class: "mono cleanup-amount", "{amount}" }
                                        if ticked && either {
                                            div { class: "segmented",
                                                role: "radiogroup",
                                                aria_label: "Move or burn the tokens of {address}",
                                                button { class: if current == TokenMove::Move { "tab on" } else { "tab" },
                                                    role: "radio",
                                                    aria_checked: "{current == TokenMove::Move}",
                                                    onclick: move |_| {
                                                        let mut next = base.clone();
                                                        next.tokens.insert(address, TokenMove::Move);
                                                        on_change.call(next);
                                                    },
                                                    "Move"
                                                }
                                                button { class: if current == TokenMove::Burn { "tab on" } else { "tab" },
                                                    role: "radio",
                                                    aria_checked: "{current == TokenMove::Burn}",
                                                    onclick: move |_| {
                                                        let mut next = base2.clone();
                                                        next.tokens.insert(address, TokenMove::Burn);
                                                        on_change.call(next);
                                                    },
                                                    "Burn"
                                                }
                                            }
                                        } else if ticked && !what.is_empty() {
                                            span { class: "muted", "{what}" }
                                        }
                                        if let Some(why) = &blocked {
                                            span { class: "chip warn", title: "{why}", "{why}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if !snapshot.stakes.is_empty() {
            Group {
                title: "Stake accounts",
                on: stakes_on,
                of: snapshot.stakes.len(),
                hint: "Both authorities, staker and withdrawer, go to the new address. The stake stays where it is, delegated or not.",
                on_toggle: all_stakes,
                ul { class: "book-list",
                    for stake in snapshot.stakes.iter().cloned() {
                        {
                            let address = stake.pubkey;
                            let picked = choices.stakes.get(&address).copied().unwrap_or_default();
                            let ticked = picked.staker || picked.withdrawer;
                            let outcome = plan.stakes.iter().find(|(pk, _)| *pk == address).map(|(_, o)| o.clone());
                            let reason = match &outcome {
                                Some(StakeOutcome::Left(why)) if *why != StakeBlocked::Chosen => Some(why.reason()),
                                Some(StakeOutcome::Handed { withdrawer: false, .. }) if ticked => {
                                    Some("Only the staker moves: this wallet cannot hand over the withdrawer.".to_string())
                                }
                                Some(StakeOutcome::Handed { staker: false, .. }) if ticked => {
                                    Some("Only the withdrawer moves.".to_string())
                                }
                                _ => None,
                            };
                            let base = choices.clone();
                            rsx! {
                                li { class: "book-row pick-row", key: "{address}",
                                    input {
                                        r#type: "checkbox",
                                        checked: ticked,
                                        title: "Hand this stake account over",
                                        aria_label: "Hand over the stake account {address}",
                                        onchange: move |evt| {
                                            let mut next = base.clone();
                                            next.stakes.insert(
                                                address,
                                                if evt.checked() { StakeChoice::BOTH } else { StakeChoice::default() },
                                            );
                                            on_change.call(next);
                                        },
                                    }
                                    AddressRow { pubkey: address,
                                        span { class: "mono cleanup-amount", "{format_sol(stake.lamports)}" }
                                    }
                                    if let Some(reason) = reason {
                                        p { class: "hint", "{reason}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(why) = &snapshot.stakes_unread {
            p { class: "note", "Stake accounts could not be listed, so none are handed over: {why}" }
        }
        if !snapshot.nonces.is_empty() {
            Group {
                title: "Nonce accounts",
                on: nonces_on,
                of: closable.len(),
                hint: "Each is closed, and its rent goes back to the wallet.",
                disabled: closable.is_empty(),
                on_toggle: all_nonces,
                ul { class: "book-list",
                    for nonce in snapshot.nonces.iter().cloned() {
                        {
                            let address = nonce.address;
                            let outcome = plan.nonces.iter().find(|(pk, _)| *pk == address).map(|(_, o)| o.clone());
                            let blocked = match &outcome {
                                Some(NonceOutcome::Left(why)) if *why != NonceBlocked::Chosen => Some(why.reason()),
                                _ => None,
                            };
                            let ticked = choices.nonces.contains(&address);
                            let is_blocked = blocked.is_some();
                            let base = choices.clone();
                            rsx! {
                                li { class: "book-row pick-row", key: "{address}",
                                    input {
                                        r#type: "checkbox",
                                        checked: ticked,
                                        disabled: is_blocked,
                                        title: "Close this nonce account",
                                        aria_label: "Close the nonce account {address}",
                                        onchange: move |evt| {
                                            let mut next = base.clone();
                                            if evt.checked() {
                                                next.nonces.insert(address);
                                            } else {
                                                next.nonces.remove(&address);
                                            }
                                            on_change.call(next);
                                        },
                                    }
                                    AddressRow { pubkey: address,
                                        span { class: "mono cleanup-amount", "{format_sol(nonce.lamports)}" }
                                    }
                                    if let Some(reason) = blocked {
                                        p { class: "hint", "{reason}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if snapshot.nonces_unread.is_some() {
            p { class: "hint", "This node does not list nonce accounts, so any this wallet controls besides its own stay as they are." }
        }
        Group {
            title: "SOL",
            on: usize::from(choices.sol),
            of: 1,
            hint: "",
            disabled: plan.sweep_estimate == 0,
            on_toggle: sol_toggle,
            p { class: "migrate-sol",
                if plan.sweep_estimate == 0 {
                    "No SOL is left to send."
                } else if choices.sol {
                    "About {format_sol(plan.sweep_estimate)} goes to "
                    span { class: "mono", "{short_pubkey(&to)}" }
                    " in the last step, worked out exactly when that step is built."
                } else {
                    "The SOL stays in the wallet."
                }
            }
        }
        if !session.is_unlocked() {
            p { class: "note", "Signing asks for the vault's password." }
        }
    }
}

/// One group of the review: a box for all of it, how many of its items are
/// ticked, and the items.
#[component]
fn Group(
    title: &'static str,
    on: usize,
    of: usize,
    hint: &'static str,
    #[props(default)] disabled: bool,
    on_toggle: EventHandler<bool>,
    children: Element,
) -> Element {
    let count = if of > 1 {
        format!("{on} of {of}")
    } else {
        String::new()
    };
    rsx! {
        section { class: if on > 0 { "migrate-group on" } else { "migrate-group" },
            label { class: "check migrate-group-head",
                input {
                    r#type: "checkbox",
                    checked: on > 0,
                    disabled,
                    onchange: move |evt| on_toggle.call(evt.checked()),
                }
                span { class: "migrate-group-title", "{title}" }
                if !count.is_empty() {
                    span { class: "muted", "{count}" }
                }
            }
            if !hint.is_empty() && on > 0 {
                p { class: "hint", "{hint}" }
            }
            {children}
        }
    }
}

/// Everything chosen moved: what still names the old wallet, and what to do
/// with it.
#[component]
fn Finished(
    from: Option<WalletId>,
    to: Option<Pubkey>,
    to_wallet: Option<WalletId>,
    references: Vec<String>,
) -> Element {
    let session = use_context::<Session>();
    let mut renamed = use_signal(|| false);
    let old = from.and_then(|id| {
        session
            .wallet_summaries()
            .into_iter()
            .find(|wallet| wallet.id == id)
    });
    let Some(old) = old else {
        return rsx! {};
    };
    let old_id = old.id;
    let moved_label = format!("{} (moved)", old.label);
    let already = old.label.ends_with("(moved)");
    rsx! {
        div { class: "callout",
            p { "Nothing you chose is left in {old.label}. Its key stays in the vault: anything sent to it later, or left behind, still needs it." }
            if !references.is_empty() {
                p { "It is still named on this device:" }
                ul {
                    for line in references.iter() {
                        li { "{line}" }
                    }
                }
            }
            div { class: "btn-row",
                if !already && !renamed() {
                    button { class: "sm",
                        onclick: move |_| {
                            let label = moved_label.clone();
                            session.when_unlocked("Unlock the vault to rename a wallet.", move || {
                                if session.rename_wallet(old_id, label.clone()).is_ok() {
                                    renamed.set(true);
                                }
                            });
                        },
                        "Rename it “{old.label} (moved)”"
                    }
                }
                if let (Some(id), Some(_)) = (to_wallet, to) {
                    if session.selected_wallet() != Some(id) {
                        button { class: "sm",
                            onclick: move |_| { let _ = session.select_wallet(id); },
                            "Use the new wallet"
                        }
                    }
                }
            }
        }
    }
}
