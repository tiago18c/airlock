//! What came of pressing Submit, over the transaction that was sent: sending,
//! waiting for it to land, then where to go from there. One that landed moves
//! to History, or off the screen. One that did not stays, to be looked at again.

use crate::state::forms::Submission;
use crate::state::session::Session;
use crate::ui::{Glyph, Icon};
use app_core::TxStatus;
use dioxus::prelude::*;
use solana_signature::Signature;

/// Where a submitted transaction stands, as the dialog says it.
enum Stage {
    /// Under way, with what is happening.
    Working(&'static str),
    /// It landed, and did what it says.
    Landed(Signature),
    /// It landed and failed: its fee was paid.
    FailedOnChain(Signature, String),
    /// It went nowhere, and why.
    DidNotLand(String),
}

fn stage(session: &Session, submission: &Submission) -> Stage {
    match submission {
        Submission::Sending => Stage::Working("Sending it to the node…"),
        Submission::Refused(error) => Stage::DidNotLand(error.clone()),
        Submission::Sent(signature) => match session.tx_status(signature) {
            None | Some(TxStatus::Sent) => Stage::Working("Sent. Waiting for it to land…"),
            Some(TxStatus::Confirmed { .. }) => Stage::Landed(*signature),
            Some(TxStatus::Failed { error }) => Stage::FailedOnChain(*signature, error),
            Some(TxStatus::Rejected { error }) => Stage::DidNotLand(error),
            Some(TxStatus::Expired) => Stage::DidNotLand(
                "Its blockhash expired before it landed. Move it to a fresh blockhash, sign it again and submit it."
                    .into(),
            ),
            Some(TxStatus::Unknown) => Stage::DidNotLand(
                "The node did not say whether it landed. If it did, History will show it. Submitting it again is safe, because one transaction cannot land twice."
                    .into(),
            ),
            Some(TxStatus::NotSent) => Stage::DidNotLand("It was not sent.".into()),
        },
    }
}

#[component]
pub(in crate::ui) fn SubmitProgress() -> Element {
    let session = use_context::<Session>();
    let form = session.forms.sign;
    let mut submission = form.submission;
    let mut error = form.error;
    let Some(current) = submission() else {
        return rsx! {};
    };
    let stage = stage(&session, &current);
    // Back on the transaction, the reason stays on screen.
    let mut back = move |why: String| {
        submission.set(None);
        error.set(Some(why));
    };
    let view = move |signature: Signature| {
        form.clear();
        session.open_in_history(signature);
    };

    rsx! {
        div { class: "modal-backdrop",
            div { class: "modal submit-progress",
                role: "dialog",
                aria_label: "Submit",
                match stage {
                    Stage::Working(what) => rsx! {
                        div { class: "submit-stage", role: "status",
                            span { class: "submit-spin", "aria-hidden": "true" }
                            p { class: "submit-title", "{what}" }
                            if let Submission::Sent(signature) = current {
                                code { class: "mono submit-sig", "{signature}" }
                                p { class: "hint", "Its toast follows it too, if this is closed." }
                                div { class: "btn-row",
                                    button { class: "ghost",
                                        onclick: move |_| submission.set(None),
                                        "Hide"
                                    }
                                }
                            }
                        }
                    },
                    Stage::Landed(signature) => rsx! {
                        div { class: "submit-stage done",
                            span { class: "submit-mark ok", Icon { glyph: Glyph::Check } }
                            p { class: "submit-title", "It landed." }
                            code { class: "mono submit-sig", "{signature}" }
                            div { class: "btn-row",
                                button { class: "primary",
                                    onclick: move |_| view(signature),
                                    "View transaction"
                                }
                                button {
                                    onclick: move |_| form.clear(),
                                    "Clear"
                                }
                            }
                        }
                    },
                    Stage::FailedOnChain(signature, why) => rsx! {
                        div { class: "submit-stage failed", role: "alert",
                            span { class: "submit-mark danger", Icon { glyph: Glyph::Alert } }
                            p { class: "submit-title", "It landed, and failed." }
                            p { class: "error", "{why}" }
                            p { class: "hint", "Its fee was paid. Nothing else it did stayed." }
                            div { class: "btn-row",
                                button { class: "primary",
                                    onclick: move |_| back(format!("It landed, and failed: {why}")),
                                    "Back to review"
                                }
                                button {
                                    onclick: move |_| view(signature),
                                    "View transaction"
                                }
                            }
                        }
                    },
                    Stage::DidNotLand(why) => rsx! {
                        div { class: "submit-stage failed", role: "alert",
                            span { class: "submit-mark danger", Icon { glyph: Glyph::Alert } }
                            p { class: "submit-title", "It did not land." }
                            p { class: "error", "{why}" }
                            div { class: "btn-row",
                                button { class: "primary",
                                    onclick: move |_| back(why.clone()),
                                    "Back to review"
                                }
                            }
                        }
                    },
                }
            }
        }
    }
}
