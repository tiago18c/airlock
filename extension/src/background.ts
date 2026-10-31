import {
  HOST_NAME,
  MAX_PENDING,
  MAX_PENDING_PER_ORIGIN,
  MAX_REQUEST_BYTES,
  METHODS,
  QUICK_TTL_MS,
  RATE_LIMIT,
  RATE_WINDOW_MS,
  REQUEST_TTL_MS,
  isRequestId,
  jsonBytes,
  newRequestId,
  replyWindowMs,
  shortCode,
  waitsForUser,
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
/** The page waiting on each request in `pending`. */
const askers = new Map<string, chrome.runtime.Port>();
/** Requests from each origin in the current rate window. */
const asked = new Map<string, number>();

let rateWindowStart = 0;
/** The origins the app last heard about. */
let reportedPages: string | null = null;
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
    abandon(port);
  });
  port.onMessage.addListener((body: unknown) => {
    void handlePage(port, origin, body).then((response) => {
      try {
        port.postMessage(response);
      } catch {
        // The page port closed while the desktop app was deciding.
      }
    });
  });
});

async function handlePage(port: chrome.runtime.Port, origin: string, body: unknown): Promise<NativeMessage> {
  const record = body && typeof body === "object" ? (body as Record<string, unknown>) : {};
  const pageId = record.id;
  if (!isRequestId(pageId)) {
    return { v: 1, error: { code: "bad_request", message: "Missing request id." } };
  }
  const refuse = (code: string, message: string): NativeMessage => ({ v: 1, id: pageId, error: { code, message } });
  if (typeof record.method !== "string" || !isMethod(record.method)) {
    return refuse("bad_request", "Unsupported method.");
  }
  if (!withinRate(origin)) {
    return refuse("too_many_requests", "This site is asking too often.");
  }
  const waits = waitsForUser(record.method, record.params);
  if (waits && !hasRoom(origin)) {
    return refuse("too_many_requests", "Too many requests are waiting in Airlock.");
  }
  // An id of this worker's own: a page chooses its id, and another page's
  // reply must never find its way to it.
  const request: NativeRequest = {
    v: 1,
    id: newRequestId(),
    origin,
    method: record.method,
    params: record.params,
    expiresAt: Math.floor((Date.now() + (waits ? REQUEST_TTL_MS : QUICK_TTL_MS)) / 1000),
  };
  if (jsonBytes(request) > MAX_REQUEST_BYTES) {
    return refuse("bad_request", "The request is too large.");
  }
  return { ...(await sendNative(request, waits ? port : undefined)), id: pageId };
}

function withinRate(origin: string): boolean {
  const now = Date.now();
  if (now - rateWindowStart >= RATE_WINDOW_MS) {
    rateWindowStart = now;
    asked.clear();
  }
  const count = (asked.get(origin) ?? 0) + 1;
  asked.set(origin, count);
  return count <= RATE_LIMIT;
}

function hasRoom(origin: string): boolean {
  const fromSite = pending.filter((item) => item.origin === origin).length;
  return pending.length < MAX_PENDING && fromSite < MAX_PENDING_PER_ORIGIN;
}

/** The page is gone, so nobody is waiting on what it asked. The app drops those
 * requests rather than ask the user about them. A send the user approved left
 * its queue already, and goes on. */
function abandon(port: chrome.runtime.Port): void {
  for (const [id, asker] of askers) {
    if (asker !== port) continue;
    waiters.get(id)?.({ v: 1, error: { code: "expired", message: "The page closed." } });
    try {
      nativePort?.postMessage({ v: 1, id: newRequestId(), method: "cancel", params: { id } });
    } catch {
      // The host went away, and the app dropped the request with it.
    }
  }
}

/** Send `request` to the app. `asker` is the page waiting on the user for it;
 * with none, it is answered at once, or soon given up on. */
function sendNative(request: NativeRequest, asker?: chrome.runtime.Port): Promise<NativeMessage> {
  const track = asker !== undefined;
  return new Promise((resolve) => {
    let settled = false;
    const finish = (message: NativeMessage) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      waiters.delete(request.id);
      if (track) {
        askers.delete(request.id);
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
    }, replyWindowMs(request.method, request.params));

    waiters.set(request.id, finish);
    if (track) {
      askers.set(request.id, asker);
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
  reportedPages = null;
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
  const origins = [...new Set(pageOrigins.values())].sort();
  const list = JSON.stringify(origins);
  // Another page of an origin already listed tells the app nothing. A list too
  // long to send leaves it with the last one it heard, which does no harm.
  if (list === reportedPages || list.length > MAX_REQUEST_BYTES) return;
  reportedPages = list;
  try {
    port.postMessage({
      v: 1,
      id: newRequestId(),
      method: "pages",
      params: { origins },
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
      expiresAt: Math.floor((Date.now() + QUICK_TTL_MS) / 1000),
    },
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

/** Where the browser says a page is from. A sandboxed frame says "null": its
 * URL may name a site, but it has none of that site's rights, and it must not
 * ask in that site's name. Every browser the manifest allows sets this. */
function originOf(sender: chrome.runtime.MessageSender): string | null {
  return sender.origin && sender.origin !== "null" ? sender.origin : null;
}

function isMethod(value: string): value is BridgeMethod {
  return (METHODS as readonly string[]).includes(value);
}

function remove(list: chrome.runtime.Port[], port: chrome.runtime.Port): void {
  const index = list.indexOf(port);
  if (index >= 0) list.splice(index, 1);
}
