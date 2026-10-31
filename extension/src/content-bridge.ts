import { EXTENSION_SOURCE, MAX_REQUEST_BYTES, PAGE_SOURCE, isRequestId, jsonBytes, needsUserGesture } from "./protocol";

interface PageReply {
  result?: unknown;
  error?: { code: string; message: string };
}

const pending = new Map<string, (reply: PageReply) => void>();
let port: chrome.runtime.Port | null = null;

/** The port to the service worker, which the browser may stop while it is
 * idle. A new one is opened when the page next needs it. */
function openPort(): chrome.runtime.Port {
  if (port) return port;
  const opened = chrome.runtime.connect({ name: "page" });
  port = opened;
  opened.onMessage.addListener((message: PageReply & { id?: string; push?: unknown }) => {
    if (message.push) {
      window.postMessage({ source: EXTENSION_SOURCE, push: message.push }, window.location.origin);
      return;
    }
    if (!message.id) return;
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    waiter(message);
  });
  opened.onDisconnect.addListener(() => {
    port = null;
    const message = chrome.runtime.lastError?.message ?? "The extension disconnected.";
    for (const waiter of pending.values()) {
      waiter({ error: { code: "host_not_found", message } });
    }
    pending.clear();
  });
  return opened;
}

openPort();

window.addEventListener("message", (event: MessageEvent) => {
  if (event.source !== window) return;
  const data = event.data as { source?: string; id?: unknown; method?: unknown; params?: unknown } | null;
  if (!data || data.source !== PAGE_SOURCE || !isRequestId(data.id)) return;
  const id = data.id;
  const answer = (reply: PageReply) => {
    window.postMessage(
      { source: EXTENSION_SOURCE, id, result: reply.result, error: reply.error },
      window.location.origin,
    );
  };
  // Read here because this world's view of it is the browser's. A page's own
  // script could say whatever it liked.
  if (needsUserGesture(data.method, data.params) && navigator.userActivation?.hasBeenActive === false) {
    answer({ error: { code: "no_user_gesture", message: "A site can ask this only after you have used its page." } });
    return;
  }
  // The service worker checks too. This keeps a huge request from reaching it.
  if (jsonBytes(data.params) > MAX_REQUEST_BYTES) {
    answer({ error: { code: "bad_request", message: "The request is too large." } });
    return;
  }
  pending.set(id, answer);
  try {
    openPort().postMessage({ id, method: data.method, params: data.params });
  } catch (err) {
    pending.delete(id);
    const message = err instanceof Error ? err.message : "The request could not be sent.";
    answer({ error: { code: "bad_request", message } });
  }
});
