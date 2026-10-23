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
