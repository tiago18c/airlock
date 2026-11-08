use crate::platform::camera::{
    list_cameras, scan_config, set_focus, stop_scanner, CameraDevice, Facing, FocusRange,
    FocusState, ScanEvent, CAMERA_JS,
};
use crate::ui::{Glyph, Icon};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use dioxus::prelude::*;
use ur_codec::UrDecoder;
use wallet_core::protocol::{hex_id, Collected, Collector, FrameSeen, MAX_PAYLOAD};

// TODO: Revisit how objects move between devices. QR is optional, so this
// pane should also accept a pasted base64 transaction or signature, and the
// camera should start only when someone chooses to scan.
/// Scans QRS1 frames into a payload. With `on_ur`, it also reads UR codes,
/// a Keystone's, and hands over each whole one as its type and message.
#[component]
pub fn QrScanner(
    on_payload: EventHandler<Vec<u8>>,
    #[props(default)] on_ur: Option<EventHandler<(String, Vec<u8>)>>,
) -> Element {
    use_drop(stop_scanner);
    // A phone picks its front or back camera with a button on the preview;
    // elsewhere the cameras are listed by name.
    let phone = crate::ui::use_phone_layout();
    let mut facing = use_signal(|| Facing::Back);
    let mut cameras = use_signal(Vec::<CameraDevice>::new);
    let mut selected = use_signal(String::new);
    let mut scanning = use_signal(|| false);
    let mut status = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut collector = use_signal(Collector::new);
    let mut ur = use_signal(UrDecoder::default);
    let mut have = use_signal(Vec::<bool>::new);
    let mut payload = use_signal(|| None::<Vec<u8>>);
    let mut last_frame = use_signal(|| None::<String>);
    let mut insecure_hint = use_signal(|| None::<String>);
    let mut focus = use_signal(|| None::<FocusState>);

    // The cameras by name, without opening one. Empty until the page has
    // camera permission; the first scan fills it then.
    use_future(move || async move {
        if let Ok(list) = list_cameras().await {
            cameras.set(list);
        }
    });

    use_future(move || async move {
        let eval = crate::platform::eval(
            r#"
            return {
                href: location.href,
                secure: window.isSecureContext,
                hasMedia: !!(navigator.mediaDevices && navigator.mediaDevices.getUserMedia)
            };
            "#,
        );
        if let Ok(value) = eval.await {
            let href = value
                .get("href")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let secure = value
                .get("secure")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let has_media = value
                .get("hasMedia")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // The desktop window is served from dioxus://index.html on macOS
            // and Linux, and from http://dioxus.index.html on Windows.
            let desktop = href.starts_with("dioxus://") || href.contains("://dioxus.index.html");
            if desktop && !has_media {
                insecure_hint.set(Some(format!(
                    "This window has no camera (page {href}, secure context: {secure}). On macOS the camera needs the app's Info.plist, so start Airlock from a bundle made by dx serve or dx bundle, not cargo run, and allow it under Privacy & Security, Camera."
                )));
            } else if !secure || !has_media {
                insecure_hint.set(Some(format!(
                    "This page is not a secure camera origin ({href}). On this computer open http://127.0.0.1 (same port). On a phone open the HTTPS URL for this host and accept the certificate warning."
                )));
            }
        }
    });

    let mut start = move || {
        error.set(None);
        payload.set(None);
        collector.write().reset();
        ur.set(UrDecoder::default());
        have.set(Vec::new());
        last_frame.set(None);
        scanning.set(true);
        status.set("Starting camera…".into());
        let device_id = if phone { String::new() } else { selected() };
        let facing = facing();
        spawn(async move {
            let mut eval = crate::platform::eval(CAMERA_JS);
            if let Err(e) = eval.send(scan_config(&device_id, facing)) {
                error.set(Some(format!("{e}")));
                scanning.set(false);
                return;
            }
            loop {
                match eval.recv::<ScanEvent>().await {
                    Ok(ScanEvent::Started) => status.set(String::new()),
                    Ok(ScanEvent::Cameras {
                        cameras: list,
                        active,
                    }) => {
                        if !phone && selected.read().is_empty() {
                            if let Some(id) = active {
                                selected.set(id);
                            }
                        }
                        cameras.set(list);
                    }
                    Ok(ScanEvent::Focus(state)) => focus.set(Some(state)),
                    Ok(ScanEvent::Blurred) => {
                        let auto = focus.read().as_ref().is_none_or(|f| f.is_auto());
                        status.set(if auto {
                                    "A code is in view but blurred. Refocusing; hold it steady, a little farther away."
                                } else {
                                    "A code is in view but blurred. Move the focus slider until it reads."
                                }.into());
                    }
                    Ok(ScanEvent::Qr { b64 }) => {
                        let bytes = match STANDARD.decode(b64.as_bytes()) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                error.set(Some(format!("base64: {e}")));
                                continue;
                            }
                        };
                        // A UR, which only a screen that reads
                        // them takes.
                        if let Some(text) = std::str::from_utf8(&bytes)
                            .ok()
                            .filter(|text| ur_codec::is_ur(text))
                        {
                            let Some(on_ur) = on_ur else {
                                status.set(
                                    "This is a UR code, which this screen does not read.".into(),
                                );
                                continue;
                            };
                            let received = ur.write().receive(text.trim());
                            if let Err(e) = received {
                                ur.set(UrDecoder::default());
                                error.set(Some(e.to_string()));
                                continue;
                            }
                            error.set(None);
                            if let Some((known, all)) = ur.read().progress() {
                                status.set(format!("Reading the codes… {known} of {all} parts"));
                            }
                            let whole = ur.read().result();
                            match whole {
                                Ok(Some(found)) => {
                                    ur.set(UrDecoder::default());
                                    status.set("Read the code.".into());
                                    on_ur.call(found);
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    ur.set(UrDecoder::default());
                                    error.set(Some(e.to_string()));
                                }
                            }
                            continue;
                        }
                        let step = collector.write().push(&bytes);
                        match step {
                            // Unrelated codes in view, and a finished
                            // code still in view, change nothing.
                            Ok(Collected::Unrelated) => {}
                            Ok(Collected::Repeat { frame }) => {
                                last_frame.set(Some(frame_line(&frame)));
                            }
                            Ok(Collected::Collecting {
                                frame,
                                received,
                                expected,
                            }) => {
                                last_frame.set(Some(frame_line(&frame)));
                                have.set(collector.read().have());
                                status.set(format!(
                                    "Collecting frames… {received}/{expected} · {}",
                                    hex_id(&frame.transfer_id)
                                ));
                            }
                            Ok(Collected::Complete {
                                frame,
                                payload: data,
                            }) => {
                                last_frame.set(Some(frame_line(&frame)));
                                have.set(collector.read().have());
                                payload.set(Some(data.clone()));
                                status.set(
                                    "Transfer complete. Show the next code, or stop the camera."
                                        .into(),
                                );
                                on_payload.call(data);
                            }
                            Err(e) => {
                                // A refused transfer starts over.
                                have.set(collector.read().have());
                                error.set(Some(e.to_string()));
                            }
                        }
                    }
                    Ok(ScanEvent::Warn { message }) => {
                        error.set(Some(message));
                    }
                    Ok(ScanEvent::Error { message }) => {
                        error.set(Some(message));
                        scanning.set(false);
                        focus.set(None);
                        status.set("Camera stopped.".into());
                    }
                    Ok(ScanEvent::Stopped) => {
                        scanning.set(false);
                        focus.set(None);
                        if payload.read().is_none() {
                            status.set("Scanner stopped.".into());
                        }
                        break;
                    }
                    Err(e) => {
                        scanning.set(false);
                        error.set(Some(format!("{e}")));
                        status.set("Scanner stopped.".into());
                        break;
                    }
                }
            }
        });
    };
    let stop = move |_| {
        stop_scanner();
        scanning.set(false);
        focus.set(None);
        status.set("Scanner stopped.".into());
    };
    // The other camera: a running scan stops, and starts again on it once
    // the first has let go of its camera.
    let flip = move |_| {
        facing.set(facing().other());
        if scanning() {
            stop_scanner();
            spawn(async move {
                for _ in 0..60 {
                    if !scanning() {
                        break;
                    }
                    crate::rt::sleep(std::time::Duration::from_millis(50)).await;
                }
                start();
            });
        }
    };

    if phone {
        return rsx! {
            div { class: "scanner phone-scanner",
                if let Some(hint) = insecure_hint() {
                    p { class: "error", "{hint}" }
                }
                div {
                    class: if scanning() { "video-wrap live" } else { "video-wrap" },
                    video {
                        id: "qr-video",
                        autoplay: true,
                        muted: true,
                        playsinline: true,
                        "webkit-playsinline": "true",
                    }
                    canvas { id: "qr-scan-canvas" }
                    div { class: "video-mask" }
                    if scanning() {
                        button { class: "scan-overlay scan-stop",
                            title: "Stop",
                            "aria-label": "Stop scanning",
                            onclick: stop,
                            Icon { glyph: Glyph::X }
                        }
                    } else {
                        button { class: "primary scan-start", onclick: move |_| start(), "Start scanning" }
                    }
                    button { class: "scan-overlay scan-flip",
                        title: facing().switch_label(),
                        "aria-label": facing().switch_label(),
                        onclick: flip,
                        Icon { glyph: Glyph::SwitchCamera }
                    }
                    if let Some(state) = focus() {
                        FocusOverlay { state }
                    }
                }
                ScanProgress { status, last_frame, error, have }
            }
        };
    }

    rsx! {
        div { class: "scanner",
            div { class: "panel-copy",
                h3 { "Camera" }
                if on_ur.is_some() {
                    p { class: "lede wordy",
                        "Point this at the codes your Keystone shows. The scanner waits until it has read enough of them."
                    }
                } else {
                    p { class: "lede wordy",
                        "Point this at a QRS1 code. The scanner waits until it has every "
                        code { "x of n" }
                        " frame, then hands the object to this pane."
                    }
                }
                if let Some(hint) = insecure_hint() {
                    p { class: "error", "{hint}" }
                }

                div { class: "btn-row",
                    if scanning() {
                        button { class: "danger", onclick: stop, "Stop" }
                    } else {
                        button { class: "primary",
                            onclick: move |_| start(),
                            "Start scanning"
                        }
                    }
                }

                label { "Camera"
                    select {
                        value: "{selected}",
                        onchange: move |e| selected.set(e.value()),
                        option { value: "", "Default camera" }
                        for cam in cameras() {
                            option { value: "{cam.id}", "{cam.label}" }
                        }
                    }
                }
                if let Some(state) = focus() {
                    FocusControl { state }
                }

                ScanProgress { status, last_frame, error, have }
            }

            div { class: "panel-preview",
                div {
                    class: if scanning() { "video-wrap live" } else { "video-wrap" },
                    video {
                        id: "qr-video",
                        autoplay: true,
                        muted: true,
                        playsinline: true,
                        "webkit-playsinline": "true",
                    }
                    canvas { id: "qr-scan-canvas" }
                    div { class: "video-mask",
                        p { "Camera preview appears here while scanning." }
                    }
                }
                p { class: "hint wordy",
                    "Max payload is {MAX_PAYLOAD} bytes. Point the camera at the cycling QR on the other device."
                }
            }
        }
    }
}

/// What the scan has read so far, and what went wrong.
#[component]
fn ScanProgress(
    status: Signal<String>,
    last_frame: Signal<Option<String>>,
    error: Signal<Option<String>>,
    have: Signal<Vec<bool>>,
) -> Element {
    rsx! {
        if !status.read().is_empty() {
            p { class: "muted", "{status}" }
        }
        if let Some(frame) = last_frame() {
            p { class: "muted", "{frame}" }
        }
        if let Some(err) = error() {
            p { class: "error", "{err}" }
        }
        if !have.read().is_empty() {
            div { class: "dots",
                for (i, ok) in have().into_iter().enumerate() {
                    span {
                        class: if ok { "dot on" } else { "dot" },
                        title: "Frame {i + 1}",
                    }
                }
            }
        }
    }
}

/// Positions on the focus slider.
const FOCUS_STEPS: f64 = 1000.0;

/// The nearest a phone's camera is taken to focus, in metres.
const NEAREST: f64 = 0.1;

/// How the focus slider spreads the camera's focus distances. A phone's
/// camera gives them in metres, and its lens focuses evenly in dioptres
/// (one over the distance): spread in metres, everything from close up to
/// a desk away sits in the first tenth of the slider. A webcam's distances
/// are steps of its own, spread as they come.
#[derive(Clone, Copy, PartialEq, Debug)]
struct FocusScale {
    range: FocusRange,
    metres: bool,
}

impl FocusScale {
    fn new(range: FocusRange) -> Self {
        Self {
            range,
            metres: cfg!(target_os = "android"),
        }
    }

    /// One over the nearest distance. Nearer than 10 cm counts as 10 cm:
    /// the Saga's front camera says 0, and taken at its word that would put
    /// everything past a few centimetres at the far end of the slider.
    fn near_dioptres(&self) -> f64 {
        1.0 / self.range.min.max(NEAREST)
    }

    fn far_dioptres(&self) -> f64 {
        1.0 / self.range.max.max(self.range.min).max(NEAREST)
    }

    /// The distance at slider position `at`, 0 the nearest.
    fn distance(&self, at: f64) -> f64 {
        let t = (at / FOCUS_STEPS).clamp(0.0, 1.0);
        let FocusRange { min, max, step } = self.range;
        if self.metres {
            let near = self.near_dioptres();
            let dioptres = near + t * (self.far_dioptres() - near);
            (1.0 / dioptres).clamp(min, max)
        } else {
            let raw = min + t * (max - min);
            let snapped = if step > 0.0 {
                min + ((raw - min) / step).round() * step
            } else {
                raw
            };
            snapped.clamp(min, max)
        }
    }

    /// The slider position for `distance`.
    fn position(&self, distance: f64) -> f64 {
        let FocusRange { min, max, .. } = self.range;
        let t = if self.metres {
            let near = self.near_dioptres();
            let span = self.far_dioptres() - near;
            if span == 0.0 {
                0.0
            } else {
                (1.0 / distance.max(NEAREST) - near) / span
            }
        } else if max > min {
            (distance - min) / (max - min)
        } else {
            0.0
        };
        (t.clamp(0.0, 1.0) * FOCUS_STEPS).round()
    }
}

/// The focus slider, near on the left and far on the right.
#[component]
fn FocusSlider(scale: FocusScale, at: Signal<f64>, disabled: bool) -> Element {
    let mut at = at;
    rsx! {
        input {
            r#type: "range",
            "aria-label": "Focus",
            min: "0",
            max: "{FOCUS_STEPS}",
            step: "1",
            value: "{scale.position(at())}",
            disabled,
            oninput: move |e| {
                if let Ok(position) = e.value().parse::<f64>() {
                    let distance = scale.distance(position);
                    at.set(distance);
                    set_focus(Some(distance));
                }
            },
        }
    }
}

/// Autofocus, and a focus distance by hand when the camera has one. Shown
/// while a camera is open.
#[component]
fn FocusControl(state: FocusState) -> Element {
    let range = state.range;
    let middle = range.map_or(0.0, |r| (r.min + r.max) / 2.0);
    // What was asked for, not what the camera reports: some cameras report
    // the old mode for a while after a change, or never switch at all.
    let mut manual = use_signal(|| !state.can_auto());
    let at = use_signal(|| state.distance.unwrap_or(middle));

    let Some(range) = range else {
        return rsx! {
            if state.can_auto() {
                p { class: "muted wordy", "Autofocus is on." }
            }
        };
    };
    rsx! {
        div { class: "focus-control",
            if state.can_auto() {
                label { class: "check",
                    input {
                        r#type: "checkbox",
                        checked: !manual(),
                        onchange: move |e| {
                            let by_hand = !e.checked();
                            manual.set(by_hand);
                            set_focus(if by_hand { Some(at()) } else { None });
                        },
                    }
                    "Autofocus"
                }
            }
            label { "Focus"
                FocusSlider { scale: FocusScale::new(range), at, disabled: !manual() }
            }
        }
    }
}

/// The phone's focus control, on the preview's bottom edge: a button that
/// switches between autofocus and focusing by hand, and the slider while
/// focusing by hand.
#[component]
fn FocusOverlay(state: FocusState) -> Element {
    let range = state.range;
    let middle = range.map_or(0.0, |r| (r.min + r.max) / 2.0);
    let mut manual = use_signal(|| !state.can_auto());
    let at = use_signal(|| state.distance.unwrap_or(middle));
    let Some(range) = range else {
        return rsx! {};
    };
    let label = if manual() {
        "Focusing by hand. Tap for autofocus"
    } else {
        "Autofocus. Tap to focus by hand"
    };
    rsx! {
        div { class: "scan-focus",
            if state.can_auto() {
                button {
                    class: "scan-overlay",
                    title: label,
                    "aria-label": label,
                    "aria-pressed": if manual() { "true" } else { "false" },
                    onclick: move |_| {
                        let by_hand = !manual();
                        manual.set(by_hand);
                        set_focus(if by_hand { Some(at()) } else { None });
                    },
                    Icon { glyph: if manual() { Glyph::Sliders } else { Glyph::Focus } }
                }
            }
            if manual() {
                FocusSlider { scale: FocusScale::new(range), at, disabled: false }
            }
        }
    }
}

fn frame_line(frame: &FrameSeen) -> String {
    format!(
        "Got {}/{} · id {}",
        frame.index + 1,
        frame.count,
        hex_id(&frame.transfer_id)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(min: f64, max: f64, step: f64) -> FocusRange {
        FocusRange { min, max, step }
    }

    #[test]
    fn a_phone_slider_spreads_focus_evenly_in_dioptres() {
        let scale = FocusScale {
            range: range(0.1, 10.0, 0.01),
            metres: true,
        };
        assert!((scale.distance(0.0) - 0.1).abs() < 1e-9);
        assert!((scale.distance(FOCUS_STEPS) - 10.0).abs() < 1e-9);
        // Half way is 5.05 dioptres, about 20 cm: in metres it would be 5 m.
        let half = scale.distance(FOCUS_STEPS / 2.0);
        assert!((half - 1.0 / 5.05).abs() < 1e-9, "{half}");
        // A desk away (50 cm) is most of the way along, not in the first
        // tenth.
        assert!(scale.position(0.5) > 750.0);
        for at in [0.0, 120.0, 500.0, 999.0] {
            assert!((scale.position(scale.distance(at)) - at).abs() <= 1.0);
        }
    }

    #[test]
    fn the_sagas_cameras_focus_across_the_slider() {
        // As its back camera reports them.
        let back = FocusScale {
            range: range(0.1, 8.221_701_622, 0.01),
            metres: true,
        };
        let desk = back.position(0.5);
        assert!((780.0..=830.0).contains(&desk), "{desk}");
        // Its front camera says it focuses from 0.
        let front = FocusScale {
            range: range(0.0, 2.155_612_23, 0.01),
            metres: true,
        };
        let desk = front.position(0.5);
        assert!((750.0..=850.0).contains(&desk), "{desk}");
        assert!((front.distance(0.0) - 0.1).abs() < 1e-9);
    }

    #[test]
    fn a_webcam_slider_is_linear_on_its_steps() {
        let scale = FocusScale {
            range: range(0.0, 250.0, 5.0),
            metres: false,
        };
        assert_eq!(scale.distance(0.0), 0.0);
        assert_eq!(scale.distance(FOCUS_STEPS), 250.0);
        assert_eq!(scale.distance(500.0), 125.0);
        assert_eq!(scale.distance(510.0), 130.0);
        assert_eq!(scale.position(125.0), 500.0);
    }
}
