import assert from "node:assert/strict";
import { mock, test } from "node:test";
import {
  MAX_PENDING,
  MAX_PENDING_PER_ORIGIN,
  MAX_REQUEST_BYTES,
  QUICK_TTL_MS,
  RATE_LIMIT,
  RATE_WINDOW_MS,
  REQUEST_TTL_MS,
  replyWindowMs,
} from "../src/protocol";

// The service worker itself, on a browser made of these. It keeps its state
// for the whole file, so each test uses origins of its own, and leaves nothing
// waiting.

class FakeEvent {
  listeners: ((...args: never[]) => void)[] = [];
  addListener(listener: (...args: never[]) => void) {
    this.listeners.push(listener);
  }
  fire(...args: unknown[]) {
    for (const listener of this.listeners) (listener as (...args: unknown[]) => void)(...args);
  }
}

class FakePort {
  onMessage = new FakeEvent();
  onDisconnect = new FakeEvent();
  sent: any[] = [];
  disconnected = false;
  constructor(
    public name: string,
    public sender?: unknown,
  ) {}
  postMessage(message: unknown) {
    if (this.disconnected) throw new Error("Attempting to use a disconnected port object");
    this.sent.push(message);
  }
  disconnect() {
    this.disconnected = true;
  }
  /** The other end went away. */
  close() {
    this.disconnected = true;
    this.onDisconnect.fire();
  }
}

const onConnect = new FakeEvent();
let native = new FakePort("native");

(globalThis as any).chrome = {
  runtime: {
    id: "test",
    lastError: undefined,
    onConnect,
    connectNative: () => (native = new FakePort("native")),
  },
};

mock.timers.enable({ apis: ["setTimeout", "Date"] });
await import("../src/background");

/** Promises settle, with the mocked timers left alone. */
const settle = () => new Promise((resolve) => setImmediate(resolve));

function pageOf(origin: string): FakePort {
  const port = new FakePort("page", { origin, url: `${origin}/`, tab: { id: 1 } });
  onConnect.fire(port);
  return port;
}

const toApp = (method: string) => native.sent.filter((message) => message.method === method);

function nextId(page: FakePort, method: string, params?: unknown, id = "page-id"): string {
  const before = toApp(method).length;
  page.onMessage.fire({ id, method, params });
  const posted = toApp(method);
  assert.equal(posted.length, before + 1, `${method} did not reach the app`);
  return posted[posted.length - 1].id;
}

/** The origins the app last heard have a page open. */
const lastListed = () => new Set<string>(toApp("pages").at(-1).params.origins);

/** A new rate window, so that only what a test does counts. */
function newWindow() {
  mock.timers.tick(RATE_WINDOW_MS);
}

async function lastReply(page: FakePort) {
  await settle();
  return page.sent[page.sent.length - 1];
}

test("a page gets its own id back, on a request with an id it did not choose", async () => {
  newWindow();
  const page = pageOf("https://ids.test");
  const appId = nextId(page, "status", undefined, "mine");
  assert.notEqual(appId, "mine");
  assert.equal(toApp("status").at(-1).origin, "https://ids.test");

  native.onMessage.fire({ v: 1, id: appId, result: { ready: true } });
  assert.deepEqual(await lastReply(page), { v: 1, id: "mine", result: { ready: true } });
});

test("two pages using one id each get their own reply", async () => {
  newWindow();
  const first = pageOf("https://first.test");
  const second = pageOf("https://second.test");
  const firstId = nextId(first, "connect", undefined, "same");
  const secondId = nextId(second, "connect", undefined, "same");
  assert.notEqual(firstId, secondId);

  native.onMessage.fire({ v: 1, id: secondId, result: { accounts: ["second"] } });
  assert.deepEqual((await lastReply(second)).result, { accounts: ["second"] });
  assert.equal(first.sent.length, 0);

  native.onMessage.fire({ v: 1, id: firstId, result: { accounts: ["first"] } });
  assert.deepEqual((await lastReply(first)).result, { accounts: ["first"] });
});

test("a page with no origin of its own is turned away", () => {
  for (const sender of [
    // A sandboxed frame, whose URL names a site whose rights it does not have.
    { origin: "null", url: "https://good.test/user-content", tab: { id: 1 } },
    { url: "https://good.test/", tab: { id: 1 } },
  ]) {
    const port = new FakePort("page", sender);
    onConnect.fire(port);
    assert.ok(port.disconnected);
  }
  // Nor does anything but a tab get to ask.
  const port = new FakePort("page", { origin: "https://good.test" });
  onConnect.fire(port);
  assert.ok(port.disconnected);
});

test("a request too large for a frame never reaches the host", async () => {
  newWindow();
  const page = pageOf("https://large.test");
  const before = native.sent.length;
  page.onMessage.fire({ id: "big", method: "signMessage", params: { message: "x".repeat(MAX_REQUEST_BYTES) } });
  const reply = await lastReply(page);
  assert.equal(reply.id, "big");
  assert.equal(reply.error.code, "bad_request");
  assert.equal(native.sent.length, before);

  // Counted in the bytes it takes, not in the characters it has.
  page.onMessage.fire({ id: "wide", method: "signMessage", params: { message: "é".repeat(MAX_REQUEST_BYTES / 2) } });
  assert.equal((await lastReply(page)).error.code, "bad_request");
  assert.equal(native.sent.length, before);

  const fits = nextId(page, "signMessage", { message: "x".repeat(MAX_REQUEST_BYTES / 2) });
  native.onMessage.fire({ v: 1, id: fits, result: {} });
  await settle();
});

test("a site that asks too often is refused until the window passes, and only that site", async () => {
  newWindow();
  const page = pageOf("https://often.test");
  for (let n = 0; n < RATE_LIMIT; n += 1) {
    native.onMessage.fire({ v: 1, id: nextId(page, "status"), result: {} });
  }
  await settle();
  const before = toApp("status").length;
  page.onMessage.fire({ id: "over", method: "status" });
  assert.equal((await lastReply(page)).error.code, "too_many_requests");
  assert.equal(toApp("status").length, before);

  const other = pageOf("https://patient.test");
  native.onMessage.fire({ v: 1, id: nextId(other, "status"), result: {} });
  await settle();

  newWindow();
  native.onMessage.fire({ v: 1, id: nextId(page, "status"), result: {} });
  await settle();
  assert.equal(page.sent.at(-1).error, undefined);
});

test("a site can keep only so many requests waiting for the user", async () => {
  newWindow();
  const page = pageOf("https://waiting.test");
  const ids = [];
  for (let n = 0; n < MAX_PENDING_PER_ORIGIN; n += 1) {
    ids.push(nextId(page, "signTransaction", { transactions: [] }));
  }
  const before = toApp("signTransaction").length;
  page.onMessage.fire({ id: "over", method: "signTransaction", params: { transactions: [] } });
  assert.equal((await lastReply(page)).error.code, "too_many_requests");
  assert.equal(toApp("signTransaction").length, before);

  // Asking whether it is connected, or leaving, waits for nobody.
  native.onMessage.fire({ v: 1, id: nextId(page, "connect", { silent: true }), result: { accounts: [] } });
  native.onMessage.fire({ v: 1, id: nextId(page, "disconnect"), result: {} });
  native.onMessage.fire({ v: 1, id: nextId(page, "status"), result: {} });
  await settle();

  // Answering one makes room for one.
  native.onMessage.fire({ v: 1, id: ids.shift(), result: {} });
  await settle();
  ids.push(nextId(page, "signTransaction", { transactions: [] }));

  for (const id of ids) native.onMessage.fire({ v: 1, id, result: {} });
  await settle();
});

test("every site together can keep only so many waiting", async () => {
  newWindow();
  const pages = [];
  const ids = [];
  for (let n = 0; ids.length < MAX_PENDING; n += 1) {
    const page = pageOf(`https://s${n}.flood.test`);
    pages.push(page);
    for (let m = 0; m < MAX_PENDING_PER_ORIGIN && ids.length < MAX_PENDING; m += 1) {
      ids.push(nextId(page, "connect"));
    }
  }
  const late = pageOf("https://late.flood.test");
  const before = toApp("connect").length;
  late.onMessage.fire({ id: "late", method: "connect" });
  assert.equal((await lastReply(late)).error.code, "too_many_requests");
  assert.equal(toApp("connect").length, before);

  for (const id of ids) native.onMessage.fire({ v: 1, id, result: {} });
  await settle();
});

test("a page that closes cancels what it was waiting on, and nothing else", async () => {
  newWindow();
  const leaving = pageOf("https://leaving.test");
  const staying = pageOf("https://staying.test");
  const waiting = nextId(leaving, "signTransaction", { transactions: [] });
  const answered = nextId(leaving, "signTransaction", { transactions: [] });
  const quick = nextId(leaving, "status");
  const other = nextId(staying, "signTransaction", { transactions: [] });
  native.onMessage.fire({ v: 1, id: answered, result: {} });
  native.onMessage.fire({ v: 1, id: quick, result: {} });
  await settle();

  const before = toApp("cancel").length;
  leaving.close();
  const cancels = toApp("cancel").slice(before);
  assert.deepEqual(
    cancels.map((message) => message.params.id),
    [waiting],
  );
  // A cancel gets no waiter of its own: it has an id, for the app's reply.
  assert.notEqual(cancels[0].id, waiting);

  // Its room is free again, and a late reply goes nowhere.
  native.onMessage.fire({ v: 1, id: waiting, error: { code: "expired", message: "gone" } });
  await settle();

  native.onMessage.fire({ v: 1, id: other, result: {} });
  await settle();
  assert.equal(staying.sent.at(-1).error, undefined);
});

test("a request nobody has to decide is given up on soon, one for the user is not", async () => {
  newWindow();
  const page = pageOf("https://slow.test");
  nextId(page, "status", undefined, "quick");
  nextId(page, "signTransaction", { transactions: [] }, "long");
  mock.timers.tick(QUICK_TTL_MS);
  const quick = await lastReply(page);
  assert.equal(quick.id, "quick");
  assert.equal(quick.error.code, "expired");
  assert.equal(page.sent.length, 1);

  mock.timers.tick(replyWindowMs("signTransaction", {}) - QUICK_TTL_MS);
  const long = await lastReply(page);
  assert.equal(long.id, "long");
  assert.equal(long.error.code, "expired");
});

test("the app has the last word on a request for the user", () => {
  const decide = replyWindowMs("signTransaction", {});
  assert.ok(decide > REQUEST_TTL_MS);
  assert.ok(replyWindowMs("connect", {}) === decide);
  assert.equal(replyWindowMs("connect", { silent: true }), QUICK_TTL_MS);
  assert.equal(replyWindowMs("status", undefined), QUICK_TTL_MS);
});

test("a send approved as time runs out is still waited for", async () => {
  newWindow();
  const page = pageOf("https://sending.test");
  const appId = nextId(page, "signAndSendTransaction", { transactions: [] }, "send");

  // Past what the user has to decide, and far past it.
  mock.timers.tick(REQUEST_TTL_MS + 30 * 60 * 1000);
  await settle();
  assert.equal(page.sent.length, 0);

  native.onMessage.fire({ v: 1, id: appId, result: { signatures: ["sig"] } });
  assert.deepEqual((await lastReply(page)).result, { signatures: ["sig"] });

  // An app that has stopped answering is given up on in the end.
  const lost = nextId(page, "signAndSendAllTransactions", { transactions: [] }, "lost");
  assert.ok(lost);
  mock.timers.tick(replyWindowMs("signAndSendAllTransactions", {}));
  const reply = await lastReply(page);
  assert.equal(reply.id, "lost");
  assert.equal(reply.error.code, "expired");
});

test("the app hears the pages only when the list of origins changes", async () => {
  newWindow();
  // Starts the host, which is told at once.
  const first = pageOf("https://listed.test");
  native.onMessage.fire({ v: 1, id: nextId(first, "status"), result: {} });
  await settle();
  const told = () => toApp("pages").length;
  const start = told();

  const second = pageOf("https://listed.test");
  assert.equal(told(), start);
  second.close();
  assert.equal(told(), start);

  const other = pageOf("https://new-origin.test");
  assert.equal(told(), start + 1);
  assert.ok(lastListed().has("https://new-origin.test"));
  other.close();
  assert.equal(told(), start + 2);
  assert.ok(!lastListed().has("https://new-origin.test"));
});

test("a host that went away is told the pages again when a new one starts", async () => {
  newWindow();
  const page = pageOf("https://reborn.test");
  native.onMessage.fire({ v: 1, id: nextId(page, "status"), result: {} });
  await settle();
  native.close();

  // The list has not changed, and the new host has still not heard it.
  page.onMessage.fire({ id: "again", method: "status" });
  assert.ok(lastListed().has("https://reborn.test"));
  native.onMessage.fire({ v: 1, id: toApp("status").at(-1).id, result: {} });
  await settle();
});

test("whoever waits when the host goes away hears about it, under the id they used", async () => {
  newWindow();
  const page = pageOf("https://orphan.test");
  nextId(page, "signTransaction", { transactions: [] }, "orphaned");
  native.close();
  const reply = await lastReply(page);
  assert.equal(reply.id, "orphaned");
  assert.equal(reply.error.code, "app_not_running");
});
