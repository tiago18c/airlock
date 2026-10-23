import type { BridgeState } from "./protocol";

const status = document.querySelector<HTMLParagraphElement>("#status");
const pending = document.querySelector<HTMLUListElement>("#pending");
const port = chrome.runtime.connect({ name: "popup" });

port.onMessage.addListener((state: BridgeState) => {
  render(state);
});

function render(state: BridgeState): void {
  if (!status || !pending) return;
  status.className = "";
  if (state.host === "connected" && state.role === "dev-stub") {
    status.className = "warn";
    status.textContent =
      "A development listener is answering. It does not approve requests. Start the Airlock desktop app instead.";
  } else if (state.host === "connected") {
    status.className = "ok";
    status.textContent = "Connected to the Airlock desktop app.";
  } else if (state.host === "missing") {
    status.className = "bad";
    status.textContent =
      "The browser cannot find bridge-host. Run bridge-host install, then restart the browser.";
  } else {
    status.className = "warn";
    status.textContent = state.lastError ?? "The Airlock desktop app is not running.";
  }

  pending.replaceChildren();
  if (state.pending.length === 0) {
    const item = document.createElement("li");
    item.className = "muted";
    item.textContent = "No site is waiting.";
    pending.append(item);
    return;
  }
  for (const request of state.pending) {
    const item = document.createElement("li");
    const code = document.createElement("code");
    code.textContent = request.shortCode;
    const detail = document.createElement("div");
    detail.textContent = `${request.method} · ${request.origin}`;
    item.append(code, detail);
    pending.append(item);
  }
}
