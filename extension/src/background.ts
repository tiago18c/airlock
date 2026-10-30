import {
  HOST_NAME,
  METHODS,
  REQUEST_TTL_MS,
  isRequestId,
  newRequestId,
  shortCode,
  type BridgeMethod,
  type BridgeState,
  type NativeMessage,
  type NativeRequest,
  type PendingRequest,
} from "./protocol";

const pagePorts: chrome.runtime.Port[] = [];
const pageOrigins = new Map<chrome.runtime.Port, string>();
const popupPorts: chrome.runtime.Port[] = [];
const waiters = new Map<string, (message: NativeMessage) => void>();
const pending: PendingRequest[] = [];

let nativePort: chrome.runtime.Port | null = null;
let host: BridgeState["host"] = "down";
let role: string | null = null;
let lastError: string | null = null;

chrome.runtime.onConnect.addListener((port) => {
  if (port.name === "popup") {
    popupPorts.push(port);
    port.onDisconnect.addListener(() => remove(popupPorts, port));
    port.postMessage(state());
    void probeDesktop();
    return;
  }
  if (port.name !== "page" || !port.sender?.tab) {
    port.disconnect();
    return;
  }
  const origin = originOf(port.sender);
  if (!origin) {
    port.disconnect();
    return;
  }
  pagePorts.push(port);
  pageOrigins.set(port, origin);
  reportPages();
  port.onDisconnect.addListener(() => {
    remove(pagePorts, port);
    pageOrigins.delete(port);
    reportPages();
  });
  port.onMessage.addListener((body: unknown) => {
    void handlePage(origin, body).then((response) => {
      try {
        port.postMessage(response);
      } catch {
        // The page port closed while the desktop app was deciding.
      }
    });
  });
});

async function handlePage(origin: string, body: unknown): Promise<NativeMessage> {
  const record = body && typeof body === "object" ? (body as Record<string, unknown>) : {};
  if (!isRequestId(record.id)) {
    return { v: 1, error: { code: "bad_request", message: "Missing request id." } };
  }
  if (typeof record.method !== "string" || !isMethod(record.method)) {
    return { v: 1, id: record.id, error: { code: "bad_request", message: "Unsupported method." } };
  }
  return sendNative(
    {
      v: 1,
      id: record.id,
      origin,
      method: record.method,
      params: record.params,
      expiresAt: Math.floor((Date.now() + REQUEST_TTL_MS) / 1000),
    },
    record.method !== "status",
  );
}

function sendNative(request: NativeRequest, track: boolean): Promise<NativeMessage> {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (message: NativeMessage) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      waiters.delete(request.id);
      if (track) {
        const index = pending.findIndex((item) => item.id === request.id);
        if (index >= 0) pending.splice(index, 1);
        notifyPopups();
      }
      // Replies built here (timeout, host gone) carry no id. The page side
      // matches on it, so stamp the request's own.
      resolve({ ...message, id: request.id });
    };
    const timer = setTimeout(() => {
      finish({
        v: 1,
        id: request.id,
        error: { code: "expired", message: "The desktop app did not answer in time." },
      });
    }, REQUEST_TTL_MS);

    waiters.set(request.id, finish);
    if (track) {
      pending.push({
        id: request.id,
        shortCode: shortCode(request.id),
        origin: request.origin,
        method: request.method,
      });
      notifyPopups();
    }
    try {
      connectNative().postMessage(request);
    } catch (err) {
      finish({
        v: 1,
        id: request.id,
        error: {
          code: "host_not_found",
          message: err instanceof Error ? err.message : "The native host could not be started.",
        },
      });
    }
  });
}

function connectNative(): chrome.runtime.Port {
  if (nativePort) return nativePort;
  const port = chrome.runtime.connectNative(HOST_NAME);
  nativePort = port;
  port.onMessage.addListener((message: NativeMessage) => {
    if (message.method === "accountsChanged") {
      // Only that site's pages: another site must not learn its accounts.
      for (const page of [...pagePorts]) {
        if (pageOrigins.get(page) !== message.origin) continue;
        try {
          page.postMessage({ push: message });
        } catch {
          remove(pagePorts, page);
          pageOrigins.delete(page);
        }
      }
      return;
    }
    if (message.id) waiters.get(message.id)?.(message);
  });
  port.onDisconnect.addListener(() => {
    const message = chrome.runtime.lastError?.message ?? "The native host disconnected.";
    nativePort = null;
    host = /not found/i.test(message) ? "missing" : "down";
    role = null;
    lastError = message;
    const error = {
      code: host === "missing" ? "host_not_found" : "app_not_running",
      message,
    };
    for (const waiter of [...waiters.values()]) waiter({ v: 1, error });
    notifyPopups();
  });
  postPages(port);
  return port;
}

/** Tell the app which origins have a page open, so it can say which sites are
 * connected. Does not start the host: it hears this when it starts. */
function reportPages(): void {
  if (nativePort) postPages(nativePort);
}

function postPages(port: chrome.runtime.Port): void {
  try {
    port.postMessage({
      v: 1,
      id: newRequestId(),
      method: "pages",
      params: { origins: [...new Set(pageOrigins.values())] },
    });
  } catch {
    // The host went away. It hears this again when it next starts.
  }
}

async function probeDesktop(): Promise<void> {
  const message = await sendNative(
    {
      v: 1,
      id: newRequestId(),
      origin: `chrome-extension://${chrome.runtime.id}`,
      method: "status",
      expiresAt: Math.floor(Date.now() / 1000) + 15,
    },
    false,
  );
  if (message.error) {
    host = message.error.code === "host_not_found" ? "missing" : "down";
    role = null;
    lastError = message.error.message;
  } else {
    host = "connected";
    lastError = null;
    const result = message.result;
    const reported =
      result && typeof result === "object" && "role" in result ? (result as { role?: unknown }).role : undefined;
    role = typeof reported === "string" ? reported : "desktop";
  }
  notifyPopups();
}

function state(): BridgeState {
  return { host, role, lastError, pending: pending.map((item) => ({ ...item })) };
}

function notifyPopups(): void {
  const snapshot = state();
  for (const port of [...popupPorts]) {
    try {
      port.postMessage(snapshot);
    } catch {
      remove(popupPorts, port);
    }
  }
}

function originOf(sender: chrome.runtime.MessageSender): string | null {
  if (sender.origin && sender.origin !== "null") return sender.origin;
  if (!sender.url) return null;
  try {
    return new URL(sender.url).origin;
  } catch {
    return null;
  }
}

function isMethod(value: string): value is BridgeMethod {
  return (METHODS as readonly string[]).includes(value);
}

function remove(list: chrome.runtime.Port[], port: chrome.runtime.Port): void {
  const index = list.indexOf(port);
  if (index >= 0) list.splice(index, 1);
}
