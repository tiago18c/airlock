import { EXTENSION_SOURCE, PAGE_SOURCE } from "./protocol";

interface PageReply {
  result?: unknown;
  error?: { code: string; message: string };
}

const port = chrome.runtime.connect({ name: "page" });
const pending = new Map<string, (reply: PageReply) => void>();

port.onMessage.addListener((message: PageReply & { id?: string; push?: unknown }) => {
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

port.onDisconnect.addListener(() => {
  const message = chrome.runtime.lastError?.message ?? "The extension disconnected.";
  for (const waiter of pending.values()) {
    waiter({ error: { code: "host_not_found", message } });
  }
  pending.clear();
});

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
  port.postMessage({ id: data.id, method: data.method, params: data.params });
});
