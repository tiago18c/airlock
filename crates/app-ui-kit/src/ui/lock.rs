use crate::state::session::{Session, Tab};
use crate::ui::{Glyph, Icon};
use dioxus::prelude::*;
use zeroize::{Zeroize, Zeroizing};

/// What unlocking allows: the quiet line under the title of every unlock
/// card that has no reason of its own, in one wording.
pub const UNLOCK_LINE: &str = "Unlock the vault to sign with its keys and change its wallets.";

/// What stays and what waits while the vault is locked, said once on every
/// pane that says it: the Wallets pane and Portfolio, on a desktop and a
/// phone alike.
pub const LOCKED_NOTE: &str = "The vault is locked. Its wallets and balances are shown; signing, and adding, removing or changing a wallet, ask for the password.";

/// What the lock screen's Skip gives, after [`UNLOCK_LINE`]: Skip cancels
/// nothing, it opens the app with the vault locked.
pub const SKIP_LINE: &str = "Skip opens the app locked: wallets and balances show, and signing or changing a wallet asks for the password.";

/// [`LOCKED_NOTE`], as a pane's note.
#[component]
pub fn LockedNote() -> Element {
    rsx! { p { class: "note", "{LOCKED_NOTE}" } }
}

#[component]
pub fn LockScreen() -> Element {
    rsx! { VaultUnlock { show_skip: true } }
}

/// Asks for the password when something waits for the vault to be unlocked,
/// and runs it once it is. Mounted once, by the shell.
#[component]
pub fn UnlockPrompt() -> Element {
    let session = use_context::<Session>();
    let Some(request) = (session.unlock_prompt)() else {
        return rsx! {};
    };
    rsx! {
        div { class: "modal-backdrop",
            onclick: move |_| session.unlock_prompt_cancel(),
            div { class: "modal",
                onclick: move |evt| evt.stop_propagation(),
                VaultUnlock {
                    show_skip: false,
                    reason: request.why,
                    // Runs in this component's scope, which outlives the
                    // form, so what it spawns keeps running.
                    on_unlocked: move |_| session.unlock_prompt_done(),
                    on_cancel: move |_| session.unlock_prompt_cancel(),
                }
            }
        }
    }
}

/// Asked once, right after a new vault is made, where the phone can keep
/// its key behind a fingerprint: whether to unlock it that way too. Mounted
/// once, by the shell.
#[component]
pub fn FingerprintOffer() -> Element {
    use crate::state::key_guard::{turn_on, use_key_guard, OFFER_FINGERPRINT};
    let session = use_context::<Session>();
    let mut error = use_signal(|| None::<String>);
    let mut working = use_signal(|| false);
    let guard = use_key_guard().filter(|guard| !guard.enrolled());
    if !OFFER_FINGERPRINT() {
        return rsx! {};
    }
    let Some(guard) = guard else {
        return rsx! {};
    };
    let mut close = move || {
        *OFFER_FINGERPRINT.write() = false;
        error.set(None);
    };
    rsx! {
        div { class: "modal-backdrop",
            div { class: "modal", role: "dialog", "aria-label": "Fingerprint unlock",
                h3 { "Unlock with your fingerprint?" }
                p { class: "muted", "The password still works, and you can change this in Settings." }
                if let Some(err) = error() {
                    p { class: "error", "{err}" }
                }
                div { class: "btn-row",
                    button { class: "ghost", disabled: working(), onclick: move |_| close(), "Not now" }
                    button {
                        class: "primary",
                        disabled: working(),
                        onclick: move |_| {
                            let guard = guard.clone();
                            working.set(true);
                            error.set(None);
                            spawn(async move {
                                let result = turn_on(session, guard).await;
                                working.set(false);
                                match result {
                                    Ok(()) => close(),
                                    Err(Some(why)) => error.set(Some(why)),
                                    // Backed out of the fingerprint prompt:
                                    // the question stays, with Not now.
                                    Err(None) => {}
                                }
                            });
                        },
                        if working() { "Waiting for fingerprint…" } else { "Turn on" }
                    }
                }
            }
        }
    }
}

/// Create or unlock the vault. With a `reason` it is the prompt's form: it
/// says why the password is asked for, and has Cancel instead of Skip.
#[component]
pub fn VaultUnlock(
    show_skip: bool,
    #[props(default)] reason: Option<&'static str>,
    #[props(default)] on_unlocked: Option<EventHandler<()>>,
    #[props(default)] on_cancel: Option<EventHandler<()>>,
) -> Element {
    let session = use_context::<Session>();
    let initialized = session.is_initialized();
    // The lock screen of an app with no network says so, under its heading.
    let offline = crate::state::online::use_offline_build();
    // Skipping lands on what works without a vault: the address book, or on
    // a phone, whose bar lists no address book, Review and Sign.
    let skip_to = if crate::ui::use_phone_layout() {
        Tab::Sign
    } else {
        Tab::AddressBook
    };
    let mut password = use_signal(String::new);
    let mut confirm = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    // The key is being derived, which takes about a second. A form that goes
    // away meanwhile takes its task with it, so nothing is unlocked after all.
    let mut working = use_signal(|| false);
    // The button and Enter in either field do the same.
    let mut submit = move || {
        if working() {
            return;
        }
        error.set(None);
        let pw = Zeroizing::new(password());
        if initialized {
            // Whatever else it is, it is for the vault to say. One made before
            // today's floor still opens.
            if pw.is_empty() {
                error.set(Some("Enter the password.".into()));
                return;
            }
        } else {
            if let Some(problem) = wallet_core::password_problem(&pw) {
                error.set(Some(problem.into()));
                return;
            }
            if *pw != *Zeroizing::new(confirm()) {
                error.set(Some("Passwords do not match.".into()));
                return;
            }
        }
        // The vault keeps the key it derived, not the password. Typed again on a
        // retry; not kept in between.
        password.write().zeroize();
        confirm.write().zeroize();
        working.set(true);
        spawn(async move {
            let result = if initialized {
                session.unlock(pw).await
            } else {
                session.create(pw).await
            };
            working.set(false);
            match result {
                Ok(()) => {
                    if !initialized {
                        *crate::state::key_guard::OFFER_FINGERPRINT.write() = true;
                    }
                    crate::actions::network::start_refresh(session);
                    if let Some(done) = on_unlocked {
                        done.call(());
                    }
                }
                Err(err) => error.set(Some(err.to_string())),
            }
        });
    };
    // Fingerprint unlock, where the phone keeps the vault's key behind one.
    let guard =
        crate::state::key_guard::use_key_guard().filter(|guard| initialized && guard.enrolled());
    let fingerprint_offered = guard.is_some();
    let fingerprint = use_callback(move |()| {
        let Some(guard) = guard.clone() else {
            return;
        };
        if *working.peek() {
            return;
        }
        error.set(None);
        working.set(true);
        spawn(async move {
            let result = crate::state::key_guard::unlock_with_fingerprint(session, guard).await;
            working.set(false);
            match result {
                Ok(()) => {
                    crate::actions::network::start_refresh(session);
                    if let Some(done) = on_unlocked {
                        done.call(());
                    }
                }
                Err(Some(why)) => error.set(Some(why)),
                Err(None) => {}
            }
        });
    });
    // Asked once, as the form appears; the button asks again.
    use_hook(move || {
        if fingerprint_offered {
            spawn(async move { fingerprint.call(()) });
        }
    });
    let on_enter = move |e: KeyboardEvent| {
        if e.key() == Key::Enter {
            e.prevent_default();
            submit();
        }
    };

    let form = rsx! {
            div { class: "stack unlock-form",
                span { class: "avatar", Icon { glyph: Glyph::Lock } }
                h2 { if initialized { "Unlock vault" } else { "Create vault" } }
                // Every unlock card has one quiet line under its title: what
                // this unlock is for, or else what unlocking allows, and on
                // the lock screen what Skip gives; then, in an app with no
                // network, the offline line.
                if initialized || reason.is_some() {
                    p { class: "hint unlock-line",
                        {reason.unwrap_or(UNLOCK_LINE)}
                        if show_skip && initialized && reason.is_none() {
                            " {SKIP_LINE}"
                        }
                    }
                }
                if offline {
                    p { class: "hint offline-note", "Offline: this app never connects." }
                }

                label { "Password"
                    input {
                        r#type: "password",
                        value: "{password}",
                        disabled: working(),
                        oninput: move |e| password.set(e.value()),
                        onkeydown: on_enter,
                    }
                }
                if !initialized {
                    label { "Confirm password"
                        input {
                            r#type: "password",
                            value: "{confirm}",
                            disabled: working(),
                            oninput: move |e| confirm.set(e.value()),
                            onkeydown: on_enter,
                        }
                    }
                    p { class: "hint",
                        "This password is all that protects the vault if someone copies its file, and it cannot be recovered. A few random words are easy to type and hard to guess."
                    }
                }
                if let Some(err) = error() {
                    p { class: "error", "{err}" }
                }
                div { class: "btn-row",
                    if let Some(cancel) = on_cancel {
                        button { class: "ghost",
                            onclick: move |_| cancel.call(()),
                            "Cancel"
                        }
                    }
                    if show_skip {
                        button { class: "ghost",
                            onclick: move |_| {
                                session.goto(skip_to);
                                session.dismiss_gate();
                            },
                            "Skip"
                        }
                    }
                    if fingerprint_offered {
                        button {
                            disabled: working(),
                            onclick: move |_| fingerprint.call(()),
                            "Use fingerprint"
                        }
                    }
                    button { class: "primary",
                        disabled: working(),
                        onclick: move |_| submit(),
                        match (initialized, working()) {
                            (true, false) => "Unlock",
                            (true, true) => "Unlocking…",
                            (false, false) => "Create vault",
                            (false, true) => "Creating…",
                        }
                    }
                }
            }
    };
    if reason.is_some() {
        return form;
    }
    rsx! {
        section { class: "card auth", {form} }
    }
}
