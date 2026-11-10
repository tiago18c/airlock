use crate::platform::qr::render_svg;
use crate::state::forms::QrPreview;
use crate::ui::{Glyph, Icon};
use dioxus::prelude::*;
use wallet_core::protocol::{
    encode_frames, encode_payload, hex_id, payload_json, QrObject, DEFAULT_CHUNK,
};

/// How long each QR frame stays on screen.
const FRAME_MS: u32 = 450;

/// The chunk of Larger codes: about QR version 12, 65 modules a side, which
/// a camera reads from a phone's screen across a desk. More frames carry
/// the same transfer.
const LARGE_CODES_CHUNK: usize = 256;

pub fn render_previews(payload: &[u8], chunk: usize) -> Result<Vec<QrPreview>, String> {
    let raw = encode_frames(payload, chunk).map_err(|e| e.to_string())?;
    let mut prepared = Vec::with_capacity(raw.len());
    for frame in raw {
        let img = render_svg(&frame.encode())?;
        prepared.push(QrPreview {
            svg: img.svg,
            index: frame.index,
            count: frame.count,
            version: img.version,
            modules: img.modules,
            encoded_bytes: img.encoded_bytes,
            transfer_id: hex_id(&frame.transfer_id),
        });
    }
    Ok(prepared)
}

/// The frames of one transfer carrying `objects`, `chunk` bytes of it each.
pub fn object_previews(objects: &[QrObject], chunk: usize) -> Result<Vec<QrPreview>, String> {
    let payload = encode_payload(objects).map_err(|e| e.to_string())?;
    render_previews(&payload, chunk)
}

/// `objects`, one transfer, as QR codes and as text to copy. `children` go
/// in a button row under the text.
#[component]
pub fn PayloadShare(
    objects: Vec<QrObject>,
    #[props(default)] short: bool,
    #[props(default)] children: Element,
) -> Element {
    let text = payload_json(&objects).map_err(|e| e.to_string());
    rsx! {
        QrCarousel { objects }
        match text {
            Ok(text) => rsx! {
                CopyableText { label: "Text", text, short }
                div { class: "btn-row payload-actions", {children} }
            },
            Err(err) => rsx! {
                p { class: "error", "{err}" }
                div { class: "btn-row", {children} }
            },
        }
    }
}

/// `text` read-only in a box, under `label` and a "Copy text" button on the
/// label's line.
#[component]
pub fn CopyableText(label: &'static str, text: String, #[props(default)] short: bool) -> Element {
    let mut copied = use_signal(|| false);
    let copy = text.clone();
    rsx! {
        div { class: "copy-field",
            div { class: "copy-field-head",
                span { class: "copy-field-label", "{label}" }
                button { class: "sm copy-text",
                    r#type: "button",
                    onclick: move |_| {
                        let text = copy.clone();
                        copied.set(true);
                        spawn(async move {
                            let _ = crate::platform::copy_text(&text).await;
                            crate::rt::sleep(std::time::Duration::from_millis(1500)).await;
                            copied.set(false);
                        });
                    },
                    Icon { glyph: if copied() { Glyph::Check } else { Glyph::Copy } }
                    if copied() { "Copied" } else { "Copy text" }
                }
            }
            textarea {
                class: if short { "short" } else { "" },
                aria_label: "{label}",
                readonly: true,
                value: "{text}",
            }
        }
    }
}

/// Plays `objects`, one transfer, as QR frames. The frames and the timer
/// belong to this component, so both stop when it unmounts.
#[component]
pub fn QrCarousel(objects: Vec<QrObject>) -> Element {
    // A phone's screen is small: its codes start large, in more frames.
    let phone = crate::ui::use_phone_layout();
    let mut large = use_signal(move || phone);
    let frames = use_memo(use_reactive!(|(objects)| {
        let chunk = if large() {
            LARGE_CODES_CHUNK
        } else {
            DEFAULT_CHUNK
        };
        object_previews(&objects, chunk)
    }));
    let mut index = use_signal(|| 0usize);
    let mut playing = use_signal(|| true);
    // A new transfer starts again at the first frame.
    use_effect(use_reactive!(|(objects)| {
        index.set(0);
        playing.set(true);
        let _ = objects;
    }));
    use_future(move || async move {
        loop {
            crate::rt::sleep(std::time::Duration::from_millis(FRAME_MS.into())).await;
            let n = frames.peek().as_ref().map(|rows| rows.len()).unwrap_or(0);
            if *playing.peek() && n > 1 {
                let at = *index.peek();
                index.set((at + 1) % n);
            }
        }
    });
    let rendered = frames();
    let frame_count = rendered.as_ref().map(|rows| rows.len()).unwrap_or(0);
    let at = if frame_count == 0 {
        0
    } else {
        index() % frame_count
    };
    let current = rendered
        .as_ref()
        .ok()
        .and_then(|rows| rows.get(at).cloned());
    let failed = rendered.as_ref().err().cloned();

    rsx! {
        if let Some(err) = failed {
            p { class: "error", "{err}" }
        } else if let Some(frame) = current {
            div { class: "qr-wrap",
                div { class: "qr-svg", dangerous_inner_html: frame.svg }
            }
            div { class: "frame-status",
                strong { "QR {frame.index + 1} of {frame.count}" }
                span { class: "muted",
                    "v{frame.version} · {frame.modules}×{frame.modules} · {frame.encoded_bytes} bytes · id {frame.transfer_id}"
                }
            }
            div { class: "btn-row",
                button { class: if large() { "sm on" } else { "sm" },
                    aria_pressed: "{large()}",
                    title: "Fewer modules in each code, in more frames, for a camera further away",
                    onclick: move |_| {
                        index.set(0);
                        large.toggle();
                    },
                    "Larger codes"
                }
                span { class: "hint",
                    "Bigger squares, over more codes, for a camera that is further away."
                }
            }
            if frame_count > 1 {
                div { class: "btn-row",
                    button { class: "icon-btn",
                        title: "Previous frame",
                        aria_label: "Previous frame",
                        onclick: move |_| {
                            if frame_count > 0 {
                                index.set((at + frame_count - 1) % frame_count);
                            }
                        },
                        Icon { glyph: Glyph::ChevronLeft }
                    }
                    button {
                        onclick: move |_| playing.set(!playing()),
                        if playing() { "Pause" } else { "Play" }
                    }
                    button { class: "icon-btn",
                        title: "Next frame",
                        aria_label: "Next frame",
                        onclick: move |_| {
                            if frame_count > 0 {
                                index.set((at + 1) % frame_count);
                            }
                        },
                        Icon { glyph: Glyph::ChevronRight }
                    }
                }
                p { class: "hint",
                    "Leave Play on so the other device can collect every frame."
                }
            }
        }
    }
}
