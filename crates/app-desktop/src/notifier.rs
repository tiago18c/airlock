//! OS toast implementation of the core's [`Notifier`] port.

use std::thread;

use app_core::{NoticeId, Notification, Notifier};
#[cfg(target_os = "windows")]
use notify_rust::NotificationResponse;
use tokio::sync::watch;

/// A click on an OS toast: sequence bumps so repeats are distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToastClick {
    pub seq: u64,
    pub notice_id: NoticeId,
}

/// Desktop [`Notifier`]: shows an OS toast; clicks bump `clicks` so the shell
/// can reveal the window and open the panel or request modal.
#[derive(Clone)]
pub struct DesktopNotifier {
    clicks: watch::Sender<ToastClick>,
}

impl DesktopNotifier {
    /// Returns the notifier and a receiver that changes when a toast is activated.
    pub fn pair() -> (Self, watch::Receiver<ToastClick>) {
        let (tx, rx) = watch::channel(ToastClick {
            seq: 0,
            notice_id: 0,
        });
        (Self { clicks: tx }, rx)
    }

    fn bump_click(&self, notice_id: NoticeId) {
        self.clicks.send_modify(|c| {
            c.seq = c.seq.wrapping_add(1);
            c.notice_id = notice_id;
        });
    }
}

impl Notifier for DesktopNotifier {
    fn notify(&self, notification: Notification) {
        let notifier = self.clone();
        let notice_id = notification.notice_id;
        thread::spawn(move || {
            let mut builder = notify_rust::Notification::new();
            builder
                .summary(&notification.title)
                .body(&notification.body)
                .appname("Airlock")
                .timeout(notify_rust::Timeout::Milliseconds(10_000));
            #[cfg(not(target_os = "windows"))]
            {
                let _ = builder.action("default", "Open");
            }
            match builder.show() {
                // A click on a Windows toast's body is `Default`, which
                // `wait_for_action` reports the same way as a dismissal.
                #[cfg(target_os = "windows")]
                Ok(handle) => {
                    let _ = handle.wait_for_response(|response: &NotificationResponse| {
                        if matches!(
                            response,
                            NotificationResponse::Default | NotificationResponse::Action(_)
                        ) {
                            notifier.bump_click(notice_id);
                        }
                    });
                }
                #[cfg(not(target_os = "windows"))]
                Ok(handle) => {
                    handle.wait_for_action(|action| {
                        if action == "default" || action == "clicked" || action.is_empty() {
                            notifier.bump_click(notice_id);
                        }
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        target: "airlock::app",
                        error = %err,
                        "OS toast failed"
                    );
                }
            }
        });
    }
}
