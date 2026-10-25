//! The steps the transaction on screen goes through, from rebuilding to
//! submitting, with the one it is on.

use super::*;
use crate::ui::{Glyph, Icon};
use wallet_core::{SignStep, SignStepKind, SignStepState};

/// The row of steps above the transaction: behind ones green, the one the
/// user is on marked, the rest still to come.
#[component]
pub(in crate::ui) fn SignSteps(steps: Vec<SignStep>) -> Element {
    rsx! {
        ol { class: "sign-steps", aria_label: "Steps",
            for (index, step) in steps.into_iter().enumerate() {
                StepItem { step, number: index + 1 }
            }
        }
    }
}

#[component]
fn StepItem(step: SignStep, number: usize) -> Element {
    let (class, glyph) = match step.state {
        SignStepState::Done => ("done", Some(Glyph::Check)),
        SignStepState::Current => ("current", None),
        SignStepState::Blocked => ("blocked", Some(Glyph::Alert)),
        SignStepState::Running => ("running", None),
        SignStepState::Failed => ("failed", Some(Glyph::X)),
        SignStepState::Unanswered => ("unanswered", Some(Glyph::Alert)),
        SignStepState::Skipped => ("skipped", None),
        SignStepState::Upcoming => ("", None),
    };
    let current = matches!(step.state, SignStepState::Current | SignStepState::Blocked);
    let detail = detail(&step);

    rsx! {
        li {
            class: "sign-step {class}",
            "aria-current": if current { "step" },
            span { class: "sign-step-mark", "aria-hidden": "true",
                if let Some(glyph) = glyph {
                    Icon { glyph }
                } else if step.state == SignStepState::Skipped {
                    "–"
                } else {
                    "{number}"
                }
            }
            span { class: "sign-step-label", "{label(step.kind)}" }
            if let Some(detail) = detail {
                span { class: "sign-step-detail", "{detail}" }
            }
        }
    }
}

fn label(kind: SignStepKind) -> &'static str {
    match kind {
        SignStepKind::Rebuild => "Rebuild",
        SignStepKind::SignAuthorization => "Sign authorization",
        SignStepKind::CollectAuthorizations => "Collect authorizations",
        SignStepKind::Simulate => "Simulate",
        SignStepKind::SignTransaction => "Sign transaction",
        SignStepKind::CollectSignatures => "Collect signatures",
        SignStepKind::Submit => "Submit",
        SignStepKind::ReturnToSite => "Return to site",
        SignStepKind::ShareToSubmit => "Share to submit",
    }
}

/// The word under a step, so its color is never the only thing that says
/// where it stands.
fn detail(step: &SignStep) -> Option<String> {
    let collects = matches!(
        step.kind,
        SignStepKind::CollectAuthorizations | SignStepKind::CollectSignatures
    );
    Some(match step.state {
        SignStepState::Done => {
            if step.kind == SignStepKind::Simulate {
                "succeeds".into()
            } else {
                "done".into()
            }
        }
        SignStepState::Blocked => "blockhash expired".into(),
        SignStepState::Running => "running".into(),
        SignStepState::Failed => "fails".into(),
        SignStepState::Unanswered => "no answer".into(),
        SignStepState::Skipped => "offline".into(),
        SignStepState::Current | SignStepState::Upcoming => match step.progress {
            Some((signed, total)) if collects => format!("{signed} of {total} in"),
            Some((signed, total)) if total > 1 => format!("{signed} of {total} signed"),
            _ if step.state == SignStepState::Current => "next".into(),
            _ => return None,
        },
    })
}
