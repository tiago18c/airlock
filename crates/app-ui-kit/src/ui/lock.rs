use crate::state::session::{Session, Tab};
use crate::ui::{Glyph, Icon};
use dioxus::prelude::*;
use zeroize::{Zeroize, Zeroizing};

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
                    crate::actions::network::start_refresh(session);
                    if let Some(done) = on_unlocked {
                        done.call(());
                    }
                }
                Err(err) => error.set(Some(err.to_string())),
            }
        });
    };
    let on_enter = move |e: KeyboardEvent| {
        if e.key() == Key::Enter {
            e.prevent_default();
            submit();
        }
    };

    let form = rsx! {
            div { class: "stack",
                span { class: "avatar", Icon { glyph: Glyph::Lock } }
                h2 { if initialized { "Unlock vault" } else { "Create vault" } }
                if let Some(reason) = reason {
                    p { class: "lede", "{reason}" }
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
                    if show_skip {
                        button { class: "ghost",
                            onclick: move |_| {
                                session.goto(skip_to);
                                session.dismiss_gate();
                            },
                            "Skip"
                        }
                    }
                    if let Some(cancel) = on_cancel {
                        button { class: "ghost",
                            onclick: move |_| cancel.call(()),
                            "Cancel"
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
