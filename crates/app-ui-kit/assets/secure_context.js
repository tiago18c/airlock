function airlockHasCameraApi() {
  return !!(navigator.mediaDevices && navigator.mediaDevices.getUserMedia);
}

function airlockCameraApiError() {
  const host = location.hostname;
  const port = location.port ? ":" + location.port : "";
  const loopback =
    host === "localhost" ||
    host === "127.0.0.1" ||
    host === "[::1]" ||
    host === "::1";
  // The desktop window: dioxus://index.html, or http://dioxus.index.html on
  // Windows.
  if (location.protocol === "dioxus:" || host === "dioxus.index.html") {
    return (
      "This window has no camera (page " +
      location.href +
      ", secure context: " +
      window.isSecureContext +
      "). On macOS the camera needs the app's Info.plist, so start Airlock " +
      "from a bundle made by dx serve or dx bundle, not cargo run, and allow " +
      "it under Privacy & Security, Camera."
    );
  }
  if (location.protocol !== "https:" && !loopback) {
    return (
      "Browsers only expose the camera on HTTPS or localhost. " +
      "This page is " +
      location.href +
      " (secure=" +
      window.isSecureContext +
      "). On this computer open http://127.0.0.1" +
      port +
      " or https://127.0.0.1" +
      port +
      ". On a phone open https://" +
      host +
      port +
      " and accept the certificate warning, then allow the camera."
    );
  }
  return (
    "Camera API unavailable. origin=" +
    location.href +
    " secure=" +
    window.isSecureContext
  );
}
