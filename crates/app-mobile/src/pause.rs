//! Sending the page nothing while the app is paused.
//!
//! Dioxus sends the page what to draw over a WebSocket on the loopback
//! interface: one batch of edits, then nothing more until the page says it
//! applied them. A phone may drop the packets of an app in the background,
//! the ones to itself on loopback too (Android's background network rules; a
//! Galaxy does it within seconds). A batch sent then is lost, and TCP sends
//! it again only after a wait that doubles each time, up to two minutes. So
//! back in front, the page stayed as it was, taking taps, until that wait
//! ran out: seconds, sometimes far longer. The node's answers were lost the
//! same way, which is why the stall ended with the network notice and a
//! refresh.
//!
//! So nothing is sent while the app is paused. As the activity pauses, a
//! [`PauseMarker`] in the page takes the number of that pause, which goes
//! out in a batch at once, while packets still pass. The page, seeing the
//! mark, keeps its answer to that batch until the app is resumed, and Dioxus,
//! waiting for the answer, sends nothing else. On resume the page answers
//! and drawing goes on from there, over a connection with nothing stuck in
//! it.
//!
//! What runs on the app's own thread (the screens' tasks) waits with it. The
//! core, its sync jobs and requests from other apps do not: they run on
//! other threads, and the screens catch up on resume.

use dioxus::prelude::*;

/// Which pause the activity is in, for the page: 0 in front, else the count
/// of pauses so far. A count, not a flag, so the page can tell a resume that
/// reaches it before the batch that marks the pause from one that is yet to
/// come.
#[derive(Clone, Copy, PartialEq)]
pub struct PauseMark(pub(crate) Signal<u64>);

/// The mark in the page. Mount it once, anywhere: it draws nothing.
#[component]
pub fn PauseMarker(mark: PauseMark) -> Element {
    let pause = (mark.0)();
    rsx! {
        span { id: "airlock-pause", hidden: true, "data-pause": "{pause}" }
    }
}

/// Run once in the page: keep the answer to the batch that marks a pause
/// until `__airlockResume` is called with that pause's number or a later
/// one. The page also resumes by itself when it becomes visible or is
/// touched, so a resume that never arrives cannot leave it waiting.
pub(crate) const HOLD_SCRIPT: &str = r#"
const it = window.interpreter;
if (it && !it.__airlockHold) {
    it.__airlockHold = true;
    let resumed = 0;
    let held = false;
    const answer = it.markEditsFinished.bind(it);
    const pause = () => Number(document.getElementById("airlock-pause")?.getAttribute("data-pause")) || 0;
    it.markEditsFinished = () => {
        if (pause() > resumed) { held = true; return; }
        answer();
    };
    window.__airlockResume = (n) => {
        if (n > resumed) resumed = n;
        if (held) { held = false; answer(); }
    };
    document.addEventListener("visibilitychange", () => {
        if (!document.hidden) window.__airlockResume(pause());
    });
    document.addEventListener("touchstart", () => window.__airlockResume(pause()), true);
}
"#;

/// What tells the page that pause `pause` is over.
pub(crate) fn resume_script(pause: u64) -> String {
    format!("window.__airlockResume && window.__airlockResume({pause});")
}
