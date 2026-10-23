window.__airlockGeneration = (window.__airlockGeneration || 0) + 1;
const __airlockMyGen = window.__airlockGeneration;

const cfg = await dioxus.recv();
let stream = null;
let track = null;
let codec = null;
let lastB64 = "";
let pixelReadBlocked = false;
// Autofocus unless someone sets a distance by hand.
let autoFocus = true;
// When the current run of "a code is in view but does not decode" began,
// and when the camera was last asked to focus again.
let blurredSince = 0;
let lastNudge = 0;
// When a code was last in view, decoded or not, and when the last decode
// began.
let lastSeen = 0;
let lastScan = 0;
// Whether a camera frame can be copied straight into the decoder's memory
// (WebCodecs). Null until the first frame says.
let directCopy = null;

function airlockAlive() {
  return window.__airlockGeneration === __airlockMyGen;
}

function $(id) {
  return document.getElementById(id);
}

function isTransientFrameError(err) {
  const name = err && err.name ? String(err.name) : "";
  const message = err ? String(err.message || err) : "";
  return (
    name === "InvalidStateError" ||
    name === "NotReadableError" ||
    name === "AbortError" ||
    name === "SecurityError" ||
    message.includes("Invalid element or state") ||
    message.includes("tainted") ||
    message.includes("Tainted")
  );
}

function bytesToB64(bytes) {
  let bin = "";
  const step = 0x8000;
  for (let i = 0; i < bytes.length; i += step) {
    bin += String.fromCharCode.apply(null, bytes.subarray(i, i + step));
  }
  return btoa(bin);
}

// The decoder is qr-codec compiled to WebAssembly (crates/qr-codec-wasm).
// Its bytes arrive with cfg; the compiled module is kept for the next scan.
// It imports nothing, so it can reach nothing outside its own memory.
async function loadCodec() {
  if (!window.__airlockCodecModule) {
    const bin = atob(cfg.codec);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    window.__airlockCodecModule = await WebAssembly.compile(bytes);
  }
  const instance = await WebAssembly.instantiate(window.__airlockCodecModule, {});
  return instance.exports;
}

const BLUR_MS = 1000;
const NUDGE_EVERY_MS = 3000;
// Decodes at most 30 times a second while a code is in view, or was in the
// last second, and 10 times a second while the camera sees none.
const SCAN_MS = 1000 / 30;
const IDLE_SCAN_MS = 1000 / 10;
const SEEN_MS = 1000;
// The decoder takes up to 1920 on the long edge.
const MAX_EDGE = 1920;

function capabilities() {
  return track && track.getCapabilities ? track.getCapabilities() : {};
}

function sendFocus() {
  const caps = capabilities();
  const s = track && track.getSettings ? track.getSettings() : {};
  const d = caps.focusDistance;
  dioxus.send({
    op: "focus",
    modes: caps.focusMode || [],
    range: d && d.max > d.min ? { min: d.min, max: d.max, step: d.step || (d.max - d.min) / 100 } : null,
    mode: s.focusMode || null,
    distance: typeof s.focusDistance === "number" ? s.focusDistance : null,
  });
}

async function setFocus(cmd) {
  if (!track) return;
  const modes = capabilities().focusMode || [];
  try {
    if (cmd.mode === "auto") {
      autoFocus = true;
      if (modes.includes("continuous")) {
        await track.applyConstraints({ advanced: [{ focusMode: "continuous" }] });
      }
    } else {
      autoFocus = false;
      await track.applyConstraints({
        advanced: [{ focusMode: "manual", focusDistance: cmd.distance }],
      });
    }
  } catch (err) {
    dioxus.send({ op: "warn", message: "The camera did not take that focus setting: " + String(err) });
  }
  sendFocus();
}

// Makes an autofocus camera search again. Webcams that only have manual
// and continuous restart their search when switched between the two.
async function nudgeFocus() {
  const modes = capabilities().focusMode || [];
  try {
    if (modes.includes("single-shot")) {
      await track.applyConstraints({ advanced: [{ focusMode: "single-shot" }] });
      if (modes.includes("continuous")) {
        setTimeout(() => {
          if (autoFocus && track) {
            track.applyConstraints({ advanced: [{ focusMode: "continuous" }] }).catch(() => {});
          }
        }, 1500);
      }
    } else if (modes.includes("manual") && modes.includes("continuous")) {
      await track.applyConstraints({ advanced: [{ focusMode: "manual" }] });
      await track.applyConstraints({ advanced: [{ focusMode: "continuous" }] });
    }
  } catch (_) {
    // A camera that refuses is left as it was.
  }
}

async function sendCameras() {
  try {
    const devices = await navigator.mediaDevices.enumerateDevices();
    dioxus.send({
      op: "cameras",
      cameras: devices
        .filter((d) => d.kind === "videoinput" && d.deviceId)
        .map((d, i) => ({ id: d.deviceId, label: d.label || "Camera " + (i + 1) })),
      active: (track.getSettings && track.getSettings().deviceId) || null,
    });
  } catch (_) {
    // The list is a convenience; scanning goes on without it.
  }
}

function stopStream() {
  const owned = stream;
  stream = null;
  if (owned) {
    owned.getTracks().forEach((t) => t.stop());
  }
  const video = $("qr-video");
  if (video && owned && video.srcObject === owned) {
    video.srcObject = null;
  }
}

function grabFrame(video, canvas, ctx) {
  const w = video.videoWidth;
  const h = video.videoHeight;
  if (!w || !h || video.readyState < 2 || !ctx) return false;
  // Every pixel: dense codes need them.
  const scale = Math.min(1, MAX_EDGE / Math.max(w, h));
  const cw = Math.max(1, Math.round(w * scale));
  const ch = Math.max(1, Math.round(h * scale));
  if (canvas.width !== cw) canvas.width = cw;
  if (canvas.height !== ch) canvas.height = ch;
  ctx.drawImage(video, 0, 0, cw, ch);
  return true;
}

// Resolves on the next camera frame, so none is decoded twice, or on the
// next animation frame where the video cannot say when one arrives. Also
// resolves after a while without a frame, so a stopped camera does not
// leave the loop waiting forever.
function nextFrame(video) {
  if (!video.requestVideoFrameCallback) {
    return new Promise((resolve) => requestAnimationFrame(resolve));
  }
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      video.cancelVideoFrameCallback(handle);
      resolve();
    }, 250);
    const handle = video.requestVideoFrameCallback(() => {
      clearTimeout(timer);
      resolve();
    });
  });
}

// Copies the camera's current frame as RGBA straight into the decoder's
// memory, with no buffer in between. Returns its size, or null when this
// webview cannot convert to RGBA or the frame is larger than the decoder
// takes.
async function copyFrame(video) {
  const frame = new VideoFrame(video);
  try {
    const { width, height } = frame.visibleRect;
    const len = width * height * 4;
    const options = { format: "RGBA", colorSpace: "srgb" };
    // A webview without the conversion ignores `format` and reports the
    // size of the camera's own format.
    if (Math.max(width, height) > MAX_EDGE || frame.allocationSize(options) !== len) {
      return null;
    }
    const ptr = codec.qr_input(len);
    // Take the view after qr_input: it may have grown the memory.
    await frame.copyTo(new Uint8Array(codec.memory.buffer, ptr, len), options);
    return { width, height };
  } finally {
    frame.close();
  }
}

function decoded(n) {
  if (n === -2) return "blurred";
  if (n <= 0) return null;
  return new Uint8Array(codec.memory.buffer, codec.qr_output(), n).slice();
}

// The decoded bytes, null when nothing decoded, or "blurred" when a code
// is in view but did not decode.
async function scanFrame(video, canvas, ctx) {
  if (!video.videoWidth || video.readyState < 2) return null;
  if (directCopy !== false && typeof VideoFrame === "function") {
    try {
      const size = await copyFrame(video);
      if (size) {
        directCopy = true;
        return decoded(codec.qr_scan(size.width, size.height));
      }
      directCopy = false;
    } catch (err) {
      // Once a copy has worked, a failure is about this frame.
      if (directCopy) throw err;
      directCopy = false;
    }
  }
  if (!grabFrame(video, canvas, ctx)) return null;
  let img;
  try {
    img = ctx.getImageData(0, 0, canvas.width, canvas.height);
  } catch (err) {
    if (!pixelReadBlocked) {
      pixelReadBlocked = true;
      dioxus.send({
        op: "warn",
        message:
          "Camera preview is live, but this WebView blocked reading its pixels (" +
          String(err) +
          ").",
      });
    }
    return null;
  }
  const rgba = img.data;
  const ptr = codec.qr_input(rgba.length);
  // Take the view after qr_input: it may have grown the memory.
  new Uint8Array(codec.memory.buffer, ptr, rgba.length).set(rgba);
  return decoded(codec.qr_scan(img.width, img.height));
}

try {
  const media = navigator.mediaDevices;
  if (!airlockHasCameraApi()) {
    throw new Error(airlockCameraApiError());
  }

  codec = await loadCodec();
  const videoConstraints = cfg.deviceId
    ? {
        deviceId: { exact: cfg.deviceId },
        width: { ideal: 1920 },
        height: { ideal: 1080 },
      }
    : {
        facingMode: { ideal: "environment" },
        width: { ideal: 1920 },
        height: { ideal: 1080 },
      };

  stream = await media.getUserMedia({
    video: videoConstraints,
    audio: false,
  });

  if (!airlockAlive()) {
    stopStream();
    return;
  }

  const video = $("qr-video");
  const canvas = $("qr-scan-canvas");
  if (!video || !canvas) {
    throw new Error("Camera preview elements are missing");
  }

  video.setAttribute("playsinline", "");
  video.setAttribute("webkit-playsinline", "");
  video.autoplay = true;
  video.muted = true;
  video.playsInline = true;
  video.srcObject = stream;
  try {
    await video.play();
  } catch (_) {
    // Autoplay can reject; the stream is still attached for loadeddata.
  }
  await new Promise((resolve) => {
    if (video.videoWidth > 0) {
      resolve();
      return;
    }
    video.addEventListener("loadeddata", resolve, { once: true });
    setTimeout(resolve, 1500);
  });

  if (!airlockAlive()) {
    stopStream();
    return;
  }

  dioxus.send({ op: "started", backend: "qr-codec" });

  track = stream.getVideoTracks()[0] || null;
  if (track) {
    // Some webcams start in fixed or manual focus.
    if ((capabilities().focusMode || []).includes("continuous")) {
      await track.applyConstraints({ advanced: [{ focusMode: "continuous" }] }).catch(() => {});
    }
    window.__airlockFocus = (cmd) => {
      if (airlockAlive()) setFocus(cmd);
    };
    sendFocus();
    // Opening a camera grants the page its names, so the list can fill now.
    await sendCameras();
  }

  let ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("Could not open a 2D canvas for QR scanning");

  while (airlockAlive()) {
    await nextFrame(video);
    const now = performance.now();
    const gap = now - lastSeen < SEEN_MS ? SCAN_MS : IDLE_SCAN_MS;
    // A frame a few ms early is still the one the cap is waiting for.
    if (now - lastScan < gap - 5) continue;
    lastScan = now;
    try {
      if (!codec) codec = await loadCodec();
      const bytes = await scanFrame(video, canvas, ctx);
      if (bytes) lastSeen = now;
      if (bytes === "blurred") {
        if (!blurredSince) blurredSince = now;
        if (now - blurredSince > BLUR_MS && now - lastNudge > NUDGE_EVERY_MS) {
          lastNudge = now;
          dioxus.send({ op: "blurred" });
          if (autoFocus) await nudgeFocus();
        }
      } else {
        blurredSince = 0;
        if (bytes && bytes.length) {
          const b64 = bytesToB64(bytes);
          if (b64 !== lastB64) {
            lastB64 = b64;
            dioxus.send({ op: "qr", b64 });
          }
        }
      }
    } catch (err) {
      if (err instanceof WebAssembly.RuntimeError) {
        // The decoder trapped on a frame. Its instance is unusable; the
        // next frame gets a fresh one.
        codec = null;
        dioxus.send({ op: "warn", message: "The QR decoder failed on a frame and restarted: " + String(err) });
      } else if (!isTransientFrameError(err)) {
        dioxus.send({ op: "warn", message: String(err) });
      }
    }
  }

  dioxus.send({ op: "stopped" });
} catch (err) {
  dioxus.send({ op: "error", message: String(err) });
} finally {
  if (airlockAlive()) window.__airlockFocus = null;
  track = null;
  stopStream();
}
