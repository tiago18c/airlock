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
    let mut password = use_signal(String::new);
    let mut confirm = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    // The button and Enter in either field do the same.
    let mut submit = move || {
        error.set(None);
        let pw = Zeroizing::new(password());
        if pw.len() < 8 {
            error.set(Some("Use at least 8 characters.".into()));
            return;
        }
        if !initialized && *pw != confirm() {
            error.set(Some("Passwords do not match.".into()));
            return;
        }
        let result = if initialized {
            session.unlock(&pw)
        } else {
            session.create(&pw)
        };
        match result {
            Ok(()) => {
                // The vault keeps the key it derived, not the password.
                password.write().zeroize();
                confirm.write().zeroize();
                crate::actions::network::start_refresh(session);
                if let Some(done) = on_unlocked {
                    done.call(());
                }
            }
            Err(err) => error.set(Some(err.to_string())),
        }
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
                        oninput: move |e| password.set(e.value()),
                        onkeydown: on_enter,
                    }
                }
                if !initialized {
                    label { "Confirm password"
                        input {
                            r#type: "password",
                            value: "{confirm}",
                            oninput: move |e| confirm.set(e.value()),
                            onkeydown: on_enter,
                        }
                    }
                }
                if let Some(err) = error() {
                    p { class: "error", "{err}" }
                }
                div { class: "btn-row",
                    button { class: "primary",
                        onclick: move |_| submit(),
                        if initialized { "Unlock" } else { "Create vault" }
                    }
                    if show_skip {
                        button { class: "ghost",
                            onclick: move |_| {
                                session.goto(Tab::AddressBook);
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
