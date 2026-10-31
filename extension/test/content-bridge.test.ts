import assert from "node:assert/strict";
import { test } from "node:test";
import { EXTENSION_SOURCE, MAX_REQUEST_BYTES, PAGE_SOURCE, needsUserGesture, waitsForUser } from "../src/protocol";

// The content script itself, on a page and a browser made of these.

class FakeEvent {
  listeners: ((...args: never[]) => void)[] = [];
  addListener(listener: (...args: never[]) => void) {
    this.listeners.push(listener);
  }
  fire(...args: unknown[]) {
    for (const listener of this.listeners) (listener as (...args: unknown[]) => void)(...args);
  }
}

const activation = { hasBeenActive: false };
const toWorker: any[] = [];
let refuseNextPost = false;
const port = {
  onMessage: new FakeEvent(),
  onDisconnect: new FakeEvent(),
  postMessage(message: unknown) {
    if (refuseNextPost) {
      refuseNextPost = false;
      throw new Error("Could not serialize message.");
    }
    toWorker.push(message);
  },
};

const listeners: ((event: unknown) => void)[] = [];
const toPage: any[] = [];
const page = {
  location: { origin: "https://page.test" },
  addEventListener: (_: string, listener: (event: unknown) => void) => listeners.push(listener),
  postMessage: (message: unknown) => toPage.push(message),
};

(globalThis as any).window = page;
(globalThis as any).chrome = { runtime: { lastError: undefined, connect: () => port } };
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: { userActivation: activation },
});
await import("../src/content-bridge");

function ask(id: unknown, method: string, params?: unknown) {
  for (const listener of listeners) {
    listener({ source: page, data: { source: PAGE_SOURCE, id, method, params } });
  }
}

function clear() {
  toWorker.length = 0;
  toPage.length = 0;
}

test("a page nobody has used cannot open a modal", () => {
  clear();
  activation.hasBeenActive = false;
  ask("c", "connect", { silent: false });
  ask("i", "signIn", {});
  assert.deepEqual(toWorker, []);
  assert.deepEqual(
    toPage.map((reply) => [reply.source, reply.id, reply.error.code]),
    [
      [EXTENSION_SOURCE, "c", "no_user_gesture"],
      [EXTENSION_SOURCE, "i", "no_user_gesture"],
    ],
  );
});

test("a page nobody has used can still ask what does not open one", () => {
  clear();
  activation.hasBeenActive = false;
  ask("s", "connect", { silent: true });
  ask("t", "status");
  // The site is connected already, or this goes no further.
  ask("x", "signTransaction", { transactions: [] });
  assert.deepEqual(
    toWorker.map((message) => message.id),
    ["s", "t", "x"],
  );
  assert.deepEqual(toPage, []);
});

test("a page that has been used can ask to connect", () => {
  clear();
  activation.hasBeenActive = true;
  ask("c", "connect", { silent: false });
  ask("i", "signIn", {});
  assert.deepEqual(
    toWorker.map((message) => message.id),
    ["c", "i"],
  );
  activation.hasBeenActive = false;
});

test("only true makes a connect silent, as in the desktop app", () => {
  for (const params of [undefined, null, {}, { silent: false }, { silent: "true" }, { silent: 1 }]) {
    assert.ok(needsUserGesture("connect", params), JSON.stringify(params));
    assert.ok(waitsForUser("connect", params), JSON.stringify(params));
  }
  assert.ok(!needsUserGesture("connect", { silent: true }));
  assert.ok(!waitsForUser("connect", { silent: true }));
});

test("what the user decides waits, and what the app answers at once does not", () => {
  for (const method of ["signTransaction", "signAndSendTransaction", "signAndSendAllTransactions", "signMessage", "signIn"]) {
    assert.ok(waitsForUser(method, {}), method);
  }
  for (const method of ["status", "disconnect"]) {
    assert.ok(!waitsForUser(method, {}), method);
  }
});

test("a request too large to send is answered here", () => {
  clear();
  ask("big", "signMessage", { message: "x".repeat(MAX_REQUEST_BYTES + 1) });
  assert.deepEqual(toWorker, []);
  assert.equal(toPage[0].id, "big");
  assert.equal(toPage[0].error.code, "bad_request");
});

test("a request with a bad id is ignored, so nothing waits on it for ever", () => {
  clear();
  ask("has space", "status");
  ask(42, "status");
  assert.deepEqual(toWorker, []);
  assert.deepEqual(toPage, []);
});

test("a request the browser will not send is answered, not left waiting", () => {
  clear();
  refuseNextPost = true;
  ask("cannot", "status");
  assert.equal(toPage[0].id, "cannot");
  assert.equal(toPage[0].error.code, "bad_request");

  // It left nothing behind for a later reply to find.
  clear();
  port.onMessage.fire({ id: "cannot", result: {} });
  assert.deepEqual(toPage, []);
});

test("a reply reaches the page under the request's id", () => {
  clear();
  ask("r", "status");
  port.onMessage.fire({ id: "r", result: { ready: true } });
  assert.deepEqual(toPage, [{ source: EXTENSION_SOURCE, id: "r", result: { ready: true }, error: undefined }]);
});
