/** A wallet for end-to-end tests, with no browser extension: the extension's
 * own AirlockWallet, whose calls go over a WebSocket to a relay on this
 * machine that speaks to the app. Only `?wallet=ws://127.0.0.1:<port>` turns
 * it on, and only a loopback address is accepted.
 *
 * Page to relay:  { "id": "<request id>", "method": "<bridge method>", "params": { … } }
 * Relay to page:  { "id": "<the same id>", "result": { … } }
 *            or:  { "id": "<the same id>", "error": { "code", "message", "requirement"? } }
 * A push:         { "push": { "v": 1, "method": "resolutionChanged" | "accountsChanged", "origin", "params" } }
 */

import { registerWallet } from "@wallet-standard/wallet";
import { BridgeError, newRequestId, replyWindowMs, type BridgeMethod } from "../../../extension/src/protocol";
import { AirlockWallet } from "../../../extension/src/wallet";

const LOOPBACK = new Set(["127.0.0.1", "localhost", "[::1]"]);

/** The relay's address from the page's query, or why it is refused. Null when
 * the page was opened without one. */
export function relayFromQuery(search: string): URL | Error | null {
  const value = new URLSearchParams(search).get("wallet");
  if (value === null) return null;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return new Error(`"${value}" is not an address. Use ?wallet=ws://127.0.0.1:<port>.`);
  }
  if (url.protocol !== "ws:" || !LOOPBACK.has(url.hostname)) {
    return new Error(`The dev transport connects only to ws:// on this machine (127.0.0.1, localhost or [::1]), not to ${value}.`);
  }
  return url;
}

export interface TransportEvents {
  /** Each frame in either direction, as text, for the event log. */
  frame(direction: "out" | "in", text: string): void;
  status(text: string, ok: boolean): void;
}

interface Waiting {
  resolve: (value: unknown) => void;
  reject: (error: BridgeError) => void;
  timer: ReturnType<typeof setTimeout>;
}

/** Registers an AirlockWallet that talks to the relay at `url`. */
export function registerRelayWallet(url: URL, events: TransportEvents): AirlockWallet {
  const waiting = new Map<string, Waiting>();
  let socket: WebSocket | null = null;
  let opened: Promise<WebSocket> | null = null;

  const open = (): Promise<WebSocket> => {
    if (opened) return opened;
    opened = new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      ws.addEventListener("open", () => {
        socket = ws;
        events.status(`Connected to the relay at ${url.href}`, true);
        resolve(ws);
      });
      ws.addEventListener("message", (event) => onFrame(String(event.data)));
      ws.addEventListener("close", () => {
        socket = null;
        opened = null;
        events.status(`The relay at ${url.href} is not connected. The next call tries again.`, false);
        for (const [id, item] of waiting) {
          clearTimeout(item.timer);
          item.reject(new BridgeError("app_not_running", "The relay closed the connection."));
          waiting.delete(id);
        }
        reject(new BridgeError("app_not_running", `Could not reach the relay at ${url.href}.`));
      });
    });
    return opened;
  };

  const onFrame = (text: string) => {
    events.frame("in", text);
    let message: { id?: unknown; result?: unknown; error?: unknown; push?: unknown };
    try {
      message = JSON.parse(text);
    } catch {
      return;
    }
    if (message.push) {
      wallet.receivePush(message.push);
      return;
    }
    if (typeof message.id !== "string") return;
    const item = waiting.get(message.id);
    if (!item) return;
    waiting.delete(message.id);
    clearTimeout(item.timer);
    if (message.error) item.reject(BridgeError.from(message.error as Record<string, unknown>));
    else item.resolve(message.result);
  };

  const call = async (method: BridgeMethod, params?: unknown): Promise<unknown> => {
    const ws = socket ?? (await open());
    const id = newRequestId();
    const text = JSON.stringify({ id, method, params });
    return new Promise((resolve, reject) => {
      // The same patience as the content script: the app has the last word.
      const timer = setTimeout(() => {
        waiting.delete(id);
        reject(new BridgeError("expired", "The relay did not answer in time."));
      }, replyWindowMs(method, params) + 2000);
      waiting.set(id, { resolve, reject, timer });
      events.frame("out", text);
      ws.send(text);
    });
  };

  const wallet = new AirlockWallet(call);
  registerWallet(wallet);
  void open().catch(() => undefined);
  return wallet;
}
