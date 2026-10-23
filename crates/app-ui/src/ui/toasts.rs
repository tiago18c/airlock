//! Transaction toasts at the root: notices with `toast: Up`, newest on top.

use std::collections::HashSet;
use std::time::Duration;

use app_core::{Notice, NoticeId, NoticeKind, ToastState, TxStatus};
use dioxus::prelude::*;
use wallet_core::explorer;

use crate::state::session::Session;
use crate::ui::{tx_status_chip, CopyButton};

/// How long a Confirmed toast stays before dismissing itself.
const CONFIRMED_HIDE: Duration = Duration::from_secs(10);
/// Visible toast slots before "n more".
const MAX_VISIBLE: usize = 3;
/// Signature characters kept on each side when the full line does not fit.
const SIG_SIDE: usize = 20;

/// Root host: survives pane switches. Reads the pumped notices slice.
/// `pinned` sits on top of the stack and has no dismiss button: a build puts
/// there what stays until it is dealt with, such as a waiting browser request.
#[component]
pub fn ToastHost(pinned: Option<Element>) -> Element {
    let session = use_context::<Session>();
    let mut auto_hide = use_signal(HashSet::<NoticeId>::new);

    // Confirmed toasts hide after 10 s (timer runs outside the WebView).
    use_effect(move || {
        let notices = (session.notices)();
        for notice in notices.items() {
            if notice.toast != ToastState::Up {
                continue;
            }
            let NoticeKind::Transaction { status, .. } = &notice.kind else {
                continue;
            };
            if !matches!(status, TxStatus::Confirmed { .. }) {
                continue;
            }
            let id = notice.id;
            // peek: writing the set must not re-run this effect.
            if auto_hide.peek().contains(&id) {
                continue;
            }
            auto_hide.write().insert(id);
            spawn(async move {
                crate::rt::sleep(CONFIRMED_HIDE).await;
                session.dismiss_toast(id);
            });
        }
    });

    let up: Vec<Notice> = (session.notices)()
        .items()
        .iter()
        .filter(|n| n.toast == ToastState::Up)
        .filter(|n| matches!(n.kind, NoticeKind::Transaction { .. }))
        .cloned()
        .collect();
    if up.is_empty() && pinned.is_none() {
        return rsx! {};
    }

    let extra = up.len().saturating_sub(MAX_VISIBLE);
    let visible = &up[..up.len().min(MAX_VISIBLE)];
    let more_label = if extra == 1 {
        "1 more".to_string()
    } else {
        format!("{extra} more")
    };

    rsx! {
        div { class: "toast-host", "aria-live": "polite",
            {pinned}
            for notice in visible.iter().cloned() {
                ToastCard { key: "{notice.id}", notice }
            }
            if extra > 0 {
                div { class: "toast-more", role: "status", "{more_label}" }
            }
        }
    }
}

#[component]
fn ToastCard(notice: Notice) -> Element {
    let session = use_context::<Session>();
    let id = notice.id;
    let NoticeKind::Transaction {
        signature,
        cluster,
        label,
        status,
        ..
    } = &notice.kind
    else {
        return rsx! {};
    };

    let sig = signature.to_string();
    let sig_shown = middle_ellipsis(&sig, SIG_SIDE * 2 + 1);
    let explorer_url = explorer::tx_url(signature, cluster);
    let (chip_class, chip_label, spinning) = tx_status_chip(status);
    let is_failure = matches!(status, TxStatus::Failed { .. } | TxStatus::Rejected { .. });
    let role = if is_failure { "alert" } else { "status" };

    rsx! {
        div { class: "toast", role: "{role}",
            div { class: "toast-head",
                span { class: "toast-label", "{label}" }
                button {
                    class: "ghost sm toast-dismiss",
                    r#type: "button",
                    "aria-label": "Dismiss",
                    onclick: move |_| session.dismiss_toast(id),
                    "×"
                }
            }
            code { class: "mono toast-sig", title: "{sig}", "{sig_shown}" }
            div { class: "toast-meta",
                span { class: "{chip_class}",
                    if spinning {
                        span { class: "toast-spin", "aria-hidden": "true" }
                    }
                    "{chip_label}"
                }
                div { class: "btn-row toast-actions",
                    if let Some(url) = explorer_url.clone() {
                        button {
                            class: "ghost sm",
                            r#type: "button",
                            onclick: move |_| {
                                if let Err(err) = session.open_link(&url) {
                                    session.show_problem(format!("Could not open link: {err}"));
                                }
                            },
                            "View on explorer"
                        }
                    }
                    CopyButton {
                        text: sig.clone(),
                        label: "Copy signature",
                        class: "ghost sm",
                    }
                }
            }
            if let Some(error) = status.error() {
                p { class: "toast-error", "{error}" }
            }
        }
    }
}

/// Keep the start and end of `s`, with an ellipsis in the middle when too long.
fn middle_ellipsis(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let side = max_chars.saturating_sub(1) / 2;
    if side == 0 {
        return "…".to_string();
    }
    let mut out = String::with_capacity(max_chars);
    out.extend(chars.iter().take(side));
    out.push('…');
    out.extend(chars.iter().skip(chars.len() - side));
    out
}
