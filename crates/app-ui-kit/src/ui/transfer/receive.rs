use crate::platform::camera::{
    list_cameras, scan_config, set_focus, stop_scanner, CameraDevice, FocusState, ScanEvent,
    CAMERA_JS,
};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use dioxus::prelude::*;
use wallet_core::protocol::{hex_id, Collected, Collector, FrameSeen, MAX_PAYLOAD};

// TODO: Revisit how objects move between devices. QR is optional, so this
// pane should also accept a pasted base64 transaction or signature, and the
// camera should start only when someone chooses to scan.
#[component]
pub fn QrScanner(on_payload: EventHandler<Vec<u8>>) -> Element {
    use_drop(stop_scanner);
    let mut cameras = use_signal(Vec::<CameraDevice>::new);
    let mut selected = use_signal(String::new);
    let mut scanning = use_signal(|| false);
    let mut status = use_signal(|| "Start scanning to turn the camera on.".to_string());
    let mut error = use_signal(|| None::<String>);
    let mut collector = use_signal(Collector::new);
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
            if !secure || !has_media {
                insecure_hint.set(Some(format!(
                    "This page is not a secure camera origin ({href}). On this computer open http://127.0.0.1 (same port). On a phone open the HTTPS URL for this host and accept the certificate warning."
                )));
            }
        }
    });

    rsx! {
        div { class: "scanner",
            div { class: "panel-copy",
                h3 { "Camera" }
                p { class: "lede",
                    "Point this at a QRS1 code. The scanner waits until it has every "
                    code { "x of n" }
                    " frame, then hands the object to this pane."
                }
                if let Some(hint) = insecure_hint() {
                    p { class: "error", "{hint}" }
                }

                div { class: "btn-row",
                    if scanning() {
                        button { class: "danger",
                            onclick: move |_| {
                                stop_scanner();
                                scanning.set(false);
                                focus.set(None);
                                status.set("Scanner stopped.".into());
                            },
                            "Stop"
                        }
                    } else {
                        button { class: "primary",
                            onclick: move |_| {
                                error.set(None);
                                payload.set(None);
                                collector.write().reset();
                                have.set(Vec::new());
                                last_frame.set(None);
                                scanning.set(true);
                                status.set("Starting camera…".into());
                                let device_id = selected();
                                spawn(async move {
                                    let mut eval = crate::platform::eval(CAMERA_JS);
                                    if let Err(e) = eval.send(scan_config(&device_id)) {
                                        error.set(Some(format!("{e}")));
                                        scanning.set(false);
                                        return;
                                    }
                                    loop {
                                        match eval.recv::<ScanEvent>().await {
                                            Ok(ScanEvent::Started { backend }) => {
                                                status.set(format!("Scanning with {backend}."));
                                            }
                                            Ok(ScanEvent::Cameras { cameras: list, active }) => {
                                                if selected.read().is_empty() {
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
                                                let step = collector.write().push(&bytes);
                                                match step {
                                                    // Unrelated codes in view, and a finished
                                                    // code still in view, change nothing.
                                                    Ok(Collected::Unrelated) => {}
                                                    Ok(Collected::Repeat { frame }) => {
                                                        last_frame.set(Some(frame_line(&frame)));
                                                    }
                                                    Ok(Collected::Collecting { frame, received, expected }) => {
                                                        last_frame.set(Some(frame_line(&frame)));
                                                        have.set(collector.read().have());
                                                        status.set(format!(
                                                            "Collecting frames… {received}/{expected} · {}",
                                                            hex_id(&frame.transfer_id)
                                                        ));
                                                    }
                                                    Ok(Collected::Complete { frame, payload: data }) => {
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
                            },
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

                p { class: "muted", "{status}" }
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
                p { class: "hint",
                    "Max payload is {MAX_PAYLOAD} bytes. Point the camera at the cycling QR on the other device."
                }
            }
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
    let mut at = use_signal(|| state.distance.unwrap_or(middle));

    let Some(range) = range else {
        return rsx! {
            if state.can_auto() {
                p { class: "muted", "Autofocus is on." }
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
                input {
                    r#type: "range",
                    min: "{range.min}",
                    max: "{range.max}",
                    step: "{range.step}",
                    value: "{at}",
                    disabled: !manual(),
                    oninput: move |e| {
                        if let Ok(d) = e.value().parse::<f64>() {
                            at.set(d);
                            set_focus(Some(d));
                        }
                    },
                }
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
