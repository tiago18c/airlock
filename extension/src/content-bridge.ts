import { EXTENSION_SOURCE, PAGE_SOURCE } from "./protocol";

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
  const data = event.data as { source?: string; id?: string; method?: string; params?: unknown } | null;
  if (!data || data.source !== PAGE_SOURCE || typeof data.id !== "string") return;
  pending.set(data.id, (reply) => {
    window.postMessage(
      { source: EXTENSION_SOURCE, id: data.id, result: reply.result, error: reply.error },
      window.location.origin,
    );
  });
  openPort().postMessage({ id: data.id, method: data.method, params: data.params });
});
