//! What only the online desktop shell wires up: a core that raises OS toasts
//! and takes browser requests, and the window following toast clicks and
//! second launches.

use std::sync::Arc;

use app_core::Core;
use app_ui_kit::Session;
use dioxus::prelude::*;
use tokio::sync::watch;

use crate::bridge::Bridge;
use crate::notifier::{DesktopNotifier, ToastClick};
use crate::shell::{run_hidden_notifier, toast_click_action, ToastClickAction};
use crate::window::reveal;

/// Open the core with OS toasts, attach the [`Bridge`] `main` provided, and
/// start the rule that turns core events into toasts while the window is
/// hidden. Returns the core and the clicks on its toasts. Call once, from the
/// root.
pub fn use_online_core() -> (Core, watch::Receiver<ToastClick>) {
    let bridge = use_context::<Bridge>();
    use_hook(|| {
        let (notifier, clicks) = DesktopNotifier::pair();
        let core = app_ui_kit::open_core_with(Arc::new(notifier));
        bridge.attach(core.public());
        let public = core.public();
        core.ports()
            .spawner
            .spawn(Box::pin(run_hidden_notifier(public)));
        (core, clicks)
    })
}

/// Bring the window forward when a second launch asks, and when an OS toast
/// is clicked, then open what the toast is about: the request modal for a
/// request still waiting, the notification center for anything else.
/// Provides the [`RequestModal`](crate::RequestModal). Call once, from the root.
pub fn use_online_window(
    core: CopyValue<Core>,
    session: Session,
    clicks: watch::Receiver<ToastClick>,
) {
    let bridge = use_context::<Bridge>();
    let request_modal = crate::use_request_modal();
    let window = dioxus::desktop::use_window();

    use_future({
        let window = window.clone();
        move || {
            let window = window.clone();
            let mut show = bridge.subscribe_show();
            async move {
                while show.changed().await.is_ok() {
                    let _ = show.borrow_and_update();
                    reveal(&window, &core.peek());
                }
            }
        }
    });

    use_future(move || {
        let window = window.clone();
        let mut clicks = clicks.clone();
        async move {
            while clicks.changed().await.is_ok() {
                let click = *clicks.borrow_and_update();
                if click.seq == 0 {
                    continue;
                }
                reveal(&window, &core.peek());
                match toast_click_action(&core.peek().public(), click.notice_id) {
                    ToastClickAction::OpenRequestModal => request_modal.show(),
                    ToastClickAction::OpenNotices { focus } => session.open_notices(focus),
                }
            }
        }
    });
}
