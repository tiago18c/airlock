import { registerWallet } from "@wallet-standard/wallet";
import {
  BridgeError,
  EXTENSION_SOURCE,
  PAGE_SOURCE,
  newRequestId,
  replyWindowMs,
  type BridgeErrorBody,
  type BridgeMethod,
} from "./protocol";
import { AirlockWallet } from "./wallet";

const wallet = new AirlockWallet((method, params) => callDesktop(method, params));
registerWallet(wallet);

window.addEventListener("message", (event: MessageEvent) => {
  if (event.source !== window) return;
  const data = event.data as { source?: string; push?: unknown } | null;
  if (!data || data.source !== EXTENSION_SOURCE || !data.push) return;
  wallet.receivePush(data.push);
});

function callDesktop(method: BridgeMethod, params?: unknown): Promise<unknown> {
  const id = newRequestId();
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => {
      window.removeEventListener("message", onMessage);
      reject(new BridgeError("expired", "Airlock did not answer in time."));
    }, replyWindowMs(method, params) + 2000);

    function onMessage(event: MessageEvent) {
      if (event.source !== window) return;
      const data = event.data as {
        source?: string;
        id?: string;
        result?: unknown;
        error?: Partial<BridgeErrorBody>;
      } | null;
      if (!data || data.source !== EXTENSION_SOURCE || data.id !== id) return;
      window.clearTimeout(timer);
      window.removeEventListener("message", onMessage);
      if (data.error) {
        reject(BridgeError.from(data.error));
      } else {
        resolve(data.result);
      }
    }

    window.addEventListener("message", onMessage);
    window.postMessage({ source: PAGE_SOURCE, id, method, params }, window.location.origin);
  });
}
