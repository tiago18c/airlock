//! Notification center: bell in the top bar and the panel of notices.

use app_core::{notice_row_text, BridgeRequestOutcome, Notice, NoticeKind};
use dioxus::prelude::*;
use wallet_core::explorer;

use crate::actions::unix_now;
use crate::state::session::Session;
use crate::ui::local_time;
use crate::ui::{tx_status_chip, CopyButton, Glyph, Icon};

/// Filter chips in the notification panel.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NoticeFilter {
    All,
    Transactions,
    Browser,
    Other,
}

impl NoticeFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Transactions => "Transactions",
            Self::Browser => "Browser",
            Self::Other => "Other",
        }
    }

    fn matches(self, kind: &NoticeKind) -> bool {
        match self {
            Self::All => true,
            Self::Transactions => matches!(kind, NoticeKind::Transaction { .. }),
            Self::Browser => matches!(kind, NoticeKind::BridgeRequest { .. }),
            Self::Other => matches!(
                kind,
                NoticeKind::VaultLocked { .. }
                    | NoticeKind::VaultRecovered
                    | NoticeKind::SyncFailed { .. }
                    | NoticeKind::SaveFailed { .. }
            ),
        }
    }
}

/// Bell with unread count; opens the notification panel.
#[component]
pub fn NoticeBell() -> Element {
    let session = use_context::<Session>();
    let notices = (session.notices)();
    let unread = notices.unread_count();
    let open = (session.notices_open)();
    let badge = if unread > 99 {
        "99+".to_string()
    } else if unread > 0 {
        unread.to_string()
    } else {
        String::new()
    };

    rsx! {
        div { class: "notice-bell-wrap",
            button {
                class: if open { "icon-btn sm notice-bell on" } else { "icon-btn sm notice-bell" },
                r#type: "button",
                title: "Notifications",
                aria_label: if unread > 0 {
                    format!("Notifications, {unread} unread")
                } else {
                    "Notifications".into()
                },
                aria_expanded: open,
                onclick: move |_| {
                    if open {
                        session.close_notices();
                    } else {
                        // Opening does not mark read.
                        session.open_notices(None);
                    }
                },
                Icon { glyph: Glyph::Bell }
                if !badge.is_empty() {
                    span { class: "notice-badge", "{badge}" }
                }
            }
            if open {
                NoticesPanel {}
            }
        }
    }
}

#[component]
fn NoticesPanel() -> Element {
    let session = use_context::<Session>();
    let mut filter = use_signal(|| NoticeFilter::All);
    let notices = (session.notices)();
    let focus = (session.notices_focus)();
    let now = unix_now();

    let items: Vec<Notice> = notices
        .items()
        .iter()
        .filter(|n| filter().matches(&n.kind))
        .cloned()
        .collect();
    let groups = group_by_day(&items, now);
    let unread = notices.unread_count();

    rsx! {
        // `data-drag: false`: they sit inside the top bar, but a click on
        // them is not a window move.
        div {
            class: "notice-backdrop",
            "data-drag": "false",
            onclick: move |_| session.close_notices(),
        }
        div {
            class: "notice-panel",
            "data-drag": "false",
            role: "dialog",
            aria_label: "Notifications",
            onclick: move |e| e.stop_propagation(),
            div { class: "notice-panel-head",
                h2 { "Notifications" }
                div { class: "btn-row",
                    button {
                        class: "ghost sm",
                        r#type: "button",
                        disabled: unread == 0,
                        onclick: move |_| session.notices_mark_all_read(),
                        "Mark all read"
                    }
                    button {
                        class: "ghost sm",
                        r#type: "button",
                        disabled: notices.is_empty(),
                        onclick: move |_| session.notices_clear(),
                        "Clear"
                    }
                }
            }
            div { class: "segmented notice-filters", role: "group", aria_label: "Filter",
                for option in [
                    NoticeFilter::All,
                    NoticeFilter::Transactions,
                    NoticeFilter::Browser,
                    NoticeFilter::Other,
                ] {
                    button {
                        class: if filter() == option { "tab on" } else { "tab" },
                        r#type: "button",
                        key: "{option.label()}",
                        onclick: move |_| filter.set(option),
                        "{option.label()}"
                    }
                }
            }
            div { class: "notice-list",
                if groups.is_empty() {
                    p { class: "muted notice-empty", "No notifications yet." }
                } else {
                    for (day_label, day_items) in groups {
                        div { class: "notice-day", key: "{day_label}",
                            h3 { class: "notice-day-label", "{day_label}" }
                            for notice in day_items {
                                NoticeRow {
                                    key: "{notice.id}",
                                    focused: focus == Some(notice.id),
                                    notice,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn NoticeRow(notice: Notice, focused: bool) -> Element {
    let session = use_context::<Session>();
    let id = notice.id;
    let unread = !notice.read;
    let removable = !notice.pinned();
    let row = notice_row_text(&notice);
    let mut expanded = use_signal(|| focused);
    // A toast click can focus a row that is already on screen, so follow the
    // prop, not only its first value.
    use_effect(use_reactive!(|focused| {
        if focused {
            expanded.set(true);
        }
    }));

    let is_tx = matches!(notice.kind, NoticeKind::Transaction { .. });

    rsx! {
        div {
            class: if unread {
                if focused { "notice-row unread focused" } else { "notice-row unread" }
            } else if focused {
                "notice-row focused"
            } else {
                "notice-row"
            },
            button {
                class: "notice-row-main",
                r#type: "button",
                onclick: move |_| {
                    if unread {
                        session.notices_mark_read(&[id]);
                    }
                    if is_tx {
                        expanded.set(!expanded());
                    }
                },
                span { class: "notice-row-text", "{row}" }
                span { class: "notice-row-time muted", "{local_time::clock_text(notice.created)}" }
            }
            // Shown on hover, over the time. A transaction still `Sent` has
            // none: it stays until it ends, as with Clear.
            if removable {
                button {
                    class: "ghost sm notice-dismiss",
                    r#type: "button",
                    title: "Dismiss",
                    aria_label: "Dismiss",
                    onclick: move |_| session.notices_remove(id),
                    "×"
                }
            }
            if is_tx && expanded() {
                TxNoticeDetail { notice: notice.clone() }
            }
            if !is_tx {
                NoticeKindDetail { notice: notice.clone() }
            }
        }
    }
}

#[component]
fn TxNoticeDetail(notice: Notice) -> Element {
    let session = use_context::<Session>();
    let NoticeKind::Transaction {
        flow,
        signature,
        cluster,
        label,
        status,
    } = &notice.kind
    else {
        return rsx! {};
    };

    let sig = signature.to_string();
    let explorer_url = explorer::tx_url(signature, cluster);
    let (chip_class, chip_label, spinning) = tx_status_chip(status);
    let cluster_name = cluster.name().to_string();
    let flow_id = *flow;

    rsx! {
        div { class: "notice-detail",
            div { class: "notice-detail-meta",
                span { class: "notice-detail-label", "{label}" }
                span { class: "{chip_class}",
                    if spinning {
                        span { class: "toast-spin", "aria-hidden": "true" }
                    }
                    "{chip_label}"
                }
                span { class: "chip", "{cluster_name}" }
            }
            code { class: "mono notice-sig", "{sig}" }
            div { class: "btn-row",
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
            p { class: "muted notice-flow",
                "Flow id {flow_id}. Search the log for this."
            }
            if let Some(error) = status.error() {
                p { class: "toast-error", "{error}" }
            }
        }
    }
}

#[component]
fn NoticeKindDetail(notice: Notice) -> Element {
    match &notice.kind {
        NoticeKind::BridgeRequest { id, outcome, .. } => {
            let code = bridge_protocol::request_code(id);
            let waiting = *outcome == BridgeRequestOutcome::Waiting;
            rsx! {
                div { class: "notice-detail muted",
                    span { "Request {code}" }
                    if waiting {
                        span { " · waiting" }
                    }
                }
            }
        }
        NoticeKind::VaultLocked { .. }
        | NoticeKind::VaultRecovered
        | NoticeKind::SyncFailed { .. }
        | NoticeKind::SaveFailed { .. } => rsx! {},
        NoticeKind::Transaction { .. } => rsx! {},
    }
}

fn group_by_day(items: &[Notice], now: i64) -> Vec<(String, Vec<Notice>)> {
    let today = local_time::day(now);
    let yesterday = today.previous_day();
    let mut out: Vec<(String, Vec<Notice>)> = Vec::new();
    for notice in items {
        let day = local_time::day(notice.created);
        let label = if day == today {
            "Today".to_string()
        } else if Some(day) == yesterday {
            "Yesterday".to_string()
        } else {
            local_time::date_text(notice.created)
        };
        match out.last_mut() {
            Some((last, list)) if *last == label => list.push(notice.clone()),
            _ => out.push((label, vec![notice.clone()])),
        }
    }
    out
}
