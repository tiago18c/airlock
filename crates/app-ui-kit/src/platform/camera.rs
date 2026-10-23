use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use dioxus::prelude::*;
use serde::Deserialize;

/// The camera loop. It reads its settings from [`scan_config`] first.
pub const CAMERA_JS: &str = concat!(
    include_str!("../../assets/secure_context.js"),
    include_str!("../../assets/camera.js"),
);

/// `qr-codec`'s scanner as WebAssembly, built by `scripts/build-qr-wasm.sh`
/// from `crates/qr-codec-wasm`. CI checks it matches its source.
const QR_CODEC_WASM: &[u8] = include_bytes!("../../assets/qr_codec.wasm");

/// The settings `CAMERA_JS` waits for: the camera, and the decoder module,
/// which the webview compiles once and keeps.
pub fn scan_config(device_id: &str) -> serde_json::Value {
    serde_json::json!({
        "deviceId": (!device_id.is_empty()).then_some(device_id),
        "codec": STANDARD.encode(QR_CODEC_WASM),
    })
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CameraDevice {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum ScanEvent {
    Qr {
        b64: String,
    },
    Started {
        backend: String,
    },
    /// Every camera, by name, once one is open, and the one in use.
    Cameras {
        cameras: Vec<CameraDevice>,
        #[serde(default)]
        active: Option<String>,
    },
    /// What the open camera's focus can do, and what it is set to.
    Focus(FocusState),
    /// A code has been in view for a while without decoding: most often
    /// it is out of focus. With autofocus on, the camera was asked to
    /// focus again.
    Blurred,
    Warn {
        message: String,
    },
    Error {
        message: String,
    },
    Stopped,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct FocusState {
    /// `focusMode` values the camera takes: `continuous`, `single-shot`,
    /// `manual`.
    #[serde(default)]
    pub modes: Vec<String>,
    /// The `focusDistance` range, when the camera can be focused by hand.
    #[serde(default)]
    pub range: Option<FocusRange>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub distance: Option<f64>,
}

impl FocusState {
    pub fn can_auto(&self) -> bool {
        self.modes.iter().any(|m| m == "continuous")
    }
    pub fn is_auto(&self) -> bool {
        self.mode.as_deref() != Some("manual")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct FocusRange {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

/// Asks the open camera for autofocus, or for a fixed focus distance. The
/// camera loop answers with a [`ScanEvent::Focus`].
pub fn set_focus(distance: Option<f64>) {
    let command = match distance {
        None => serde_json::json!({ "mode": "auto" }),
        Some(d) => serde_json::json!({ "mode": "manual", "distance": d }),
    };
    let _ = crate::platform::eval(format!(
        "window.__airlockFocus && window.__airlockFocus({command});"
    ));
}

#[derive(Debug, Deserialize)]
struct CameraListResult {
    ok: bool,
    #[serde(default)]
    cameras: Vec<CameraDevice>,
    #[serde(default)]
    error: Option<String>,
}

/// Lists cameras without opening one. Before the page has camera
/// permission the browser shows at most one unnamed placeholder, so the
/// list is empty until then: on the web until the first scan, on desktop
/// never, because the app grants its own page up front.
const LIST_CAMERAS_JS: &str = concat!(
    include_str!("../../assets/secure_context.js"),
    r#"
        try {
            if (!airlockHasCameraApi()) {
                return { ok: false, error: airlockCameraApiError() };
            }
            const devices = await navigator.mediaDevices.enumerateDevices();
            return {
                ok: true,
                cameras: devices
                    .filter((d) => d.kind === "videoinput" && d.deviceId && d.label)
                    .map((d) => ({ id: d.deviceId, label: d.label })),
            };
        } catch (err) {
            return { ok: false, error: String(err) };
        }
    "#,
);

/// Cameras by name, without opening any. Empty when the page has no camera
/// permission yet.
pub async fn list_cameras() -> Result<Vec<CameraDevice>, String> {
    let eval = crate::platform::eval(LIST_CAMERAS_JS);
    let value = eval.await.map_err(|e| format!("{e}"))?;
    let result: CameraListResult =
        serde_json::from_value(value).map_err(|e| format!("camera list parse failed: {e}"))?;
    if result.ok {
        Ok(result.cameras)
    } else {
        Err(result
            .error
            .unwrap_or_else(|| "unknown camera list error".into()))
    }
}

pub fn stop_scanner() {
    let _ = crate::platform::eval(
        "window.__airlockGeneration = (window.__airlockGeneration || 0) + 1;",
    );
}
