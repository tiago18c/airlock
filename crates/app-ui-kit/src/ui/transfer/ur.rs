//! UR codes, as a Keystone reads and writes them: one message in one code,
//! or a fountain of parts that goes on until the reader has enough.

use crate::platform::qr::render_svg;
use dioxus::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use ur_codec::UrEncoder;

/// How long each code stays on screen.
const FRAME_MS: u64 = 300;
/// Bytes of the message in each part: a code a phone-class camera reads
/// from a screen at arm's length.
const FRAGMENT: usize = 200;

/// The encoder of one message. Two are the same only if they are the one.
#[derive(Clone)]
struct Encoder(Rc<RefCell<Option<UrEncoder>>>);

impl PartialEq for Encoder {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Plays `message` as UR codes of `ur_type`. A message that fits one code
/// stays still; a longer one cycles through parts for as long as it is on
/// screen.
#[component]
pub fn UrCarousel(ur_type: String, message: Vec<u8>) -> Element {
    let kind = ur_type.clone();
    let encoder = use_memo(use_reactive!(|(ur_type, message)| {
        Encoder(Rc::new(RefCell::new(UrEncoder::new(
            &ur_type, &message, FRAGMENT,
        ))))
    }));
    let mut code = use_signal(String::new);
    use_effect(move || {
        let encoder = encoder();
        let next = encoder.0.borrow_mut().as_mut().map(UrEncoder::next_code);
        code.set(next.unwrap_or_default());
    });
    use_future(move || async move {
        loop {
            crate::rt::sleep(std::time::Duration::from_millis(FRAME_MS)).await;
            let encoder = encoder.peek().clone();
            let next = encoder
                .0
                .borrow_mut()
                .as_mut()
                .filter(|encoder| !encoder.is_single_part())
                .map(UrEncoder::next_code);
            if let Some(next) = next {
                code.set(next);
            }
        }
    });
    let parts = encoder()
        .0
        .borrow()
        .as_ref()
        .map_or(0, UrEncoder::fragment_count);
    let text = code();
    if text.is_empty() {
        return rsx! { p { class: "error", "There is nothing to show." } };
    }
    match render_svg(text.as_bytes()) {
        Ok(image) => rsx! {
            div { class: "qr-wrap",
                div { class: "qr-svg", dangerous_inner_html: image.svg }
            }
            div { class: "frame-status",
                if parts > 1 {
                    strong { "{parts} parts, playing" }
                } else {
                    strong { "One code" }
                }
                span { class: "muted", "UR · {kind} · v{image.version}" }
            }
        },
        Err(err) => rsx! { p { class: "error", "{err}" } },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_part_of_a_long_request_fits_a_code() {
        let mut encoder = UrEncoder::new("sol-sign-request", &[7u8; 1232], FRAGMENT).unwrap();
        assert!(!encoder.is_single_part());
        for _ in 0..encoder.fragment_count() * 2 {
            let code = encoder.next_code();
            let image = render_svg(code.as_bytes()).unwrap();
            assert!(image.version <= 20, "v{}", image.version);
        }
    }
}
