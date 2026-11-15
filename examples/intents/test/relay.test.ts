import assert from "node:assert/strict";
import { connect as connectTcp, type Socket } from "node:net";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { Keypair } from "@solana/web3.js";
import * as protocol from "../../../extension/src/protocol";
import * as relayModule from "../relay.mjs";
import { memo } from "../src/instructions";
import { legacy } from "./helpers";

const {
  FrameReader,
  OPCODE,
  Relay,
  WsReader,
  acceptKey,
  encodeFrame,
  encodeWsFrame,
  isLoopbackHost,
  isLoopbackOrigin,
  startRelay,
} = relayModule;

const FAKE_APP = fileURLToPath(new URL("../test/fake-app.mjs", import.meta.url));
const ORIGIN = "http://127.0.0.1:8124";

test("the relay allows what the extension allows", () => {
  assert.deepEqual(relayModule.METHODS, [...protocol.METHODS]);
  assert.deepEqual(relayModule.INTENT_METHODS, [...protocol.INTENT_METHODS]);
  assert.deepEqual(relayModule.PUSH_METHODS, [...protocol.PUSH_METHODS]);
  for (const name of ["REQUEST_TTL_MS", "QUICK_TTL_MS", "MAX_TRANSACTIONS", "MAX_PENDING_PER_ORIGIN", "MAX_PENDING", "RATE_LIMIT", "RATE_WINDOW_MS", "MAX_REQUEST_BYTES"] as const) {
    assert.equal(relayModule[name], protocol[name], name);
  }
  const cases: [string, unknown][] = [
    ["status", undefined],
    ["connect", { silent: true }],
    ["connect", {}],
    ["resolve", {}],
    ["resolveMessage", {}],
    ["signAndSendTransaction", {}],
    ["signMessage", {}],
    ["disconnect", {}],
  ];
  for (const [method, params] of cases) {
    assert.equal(relayModule.waitsForUser(method, params), protocol.waitsForUser(method, params), method);
    assert.equal(relayModule.replyWindowMs(method, params), protocol.replyWindowMs(method, params), method);
  }
});

test("native messaging frames read back across any split, several to a chunk", () => {
  const messages = [{ v: 1, id: "a", result: { text: "héllo" } }, { v: 1, method: "resolutionChanged", origin: ORIGIN }];
  const bytes = Buffer.concat(messages.map((message) => encodeFrame(message)));
  assert.equal(bytes.readUInt32LE(0), Buffer.byteLength(JSON.stringify(messages[0])));
  for (let cut = 0; cut <= bytes.length; cut += 1) {
    const reader = new FrameReader();
    const got = [...reader.push(bytes.subarray(0, cut)), ...reader.push(bytes.subarray(cut))];
    assert.deepEqual(got, messages, `cut at ${cut}`);
  }
  const huge = Buffer.alloc(4);
  huge.writeUInt32LE(2 * 1024 * 1024, 0);
  assert.throws(() => new FrameReader().push(huge), /frame of/);
});

test("WebSocket frames: the handshake key, masked and unmasked, and every length form", () => {
  // RFC 6455, section 1.3.
  assert.equal(acceptKey("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
  for (const size of [0, 5, 125, 126, 65535, 65536, 200_000]) {
    const text = "x".repeat(size);
    const masked = encodeWsFrame(OPCODE.text, text, Buffer.from([1, 2, 3, 4]));
    const frames = new WsReader().push(masked);
    assert.equal(frames.length, 1);
    assert.equal(frames[0].payload.toString(), text, `size ${size}`);
    const plain = new WsReader({ requireMask: false }).push(encodeWsFrame(OPCODE.text, text));
    assert.equal(plain[0].payload.length, size);
  }
  // Split in two, and two in one.
  const two = Buffer.concat([encodeWsFrame(OPCODE.text, "one", Buffer.from([9, 9, 9, 9])), encodeWsFrame(OPCODE.ping, "", Buffer.from([1, 1, 1, 1]))]);
  const reader = new WsReader();
  const got = [...reader.push(two.subarray(0, 5)), ...reader.push(two.subarray(5))];
  assert.deepEqual(got.map((frame: { opcode: number }) => frame.opcode), [OPCODE.text, OPCODE.ping]);
  assert.throws(() => new WsReader().push(encodeWsFrame(OPCODE.text, "x")), /unmasked/);
  const fragment = encodeWsFrame(OPCODE.text, "x", Buffer.from([1, 2, 3, 4]));
  fragment[0] &= 0x7f;
  assert.throws(() => new WsReader().push(fragment), /fragmented/);
});

test("only a page on this machine may connect", () => {
  for (const good of ["http://127.0.0.1:8124", "http://localhost:3000", "http://[::1]:8124", "https://localhost"]) assert.ok(isLoopbackOrigin(good), good);
  for (const bad of ["https://app.example", "http://127.0.0.2:8124", "null", "", "http://localhost.evil.test", "http://127.0.0.1:8124/path", "chrome-extension://abc", undefined]) {
    assert.ok(!isLoopbackOrigin(bad), String(bad));
  }
  assert.ok(isLoopbackHost("127.0.0.1:9000") && isLoopbackHost("localhost") && isLoopbackHost("[::1]:9000"));
  assert.ok(!isLoopbackHost("rebound.evil.test:9000") && !isLoopbackHost(undefined));
});

/** A relay over an in-memory host that records what it is sent. */
function harness(replyWindow?: (method: string, params: unknown) => number) {
  const toHost: Record<string, unknown>[] = [];
  let onMessage: (message: unknown) => void = () => {};
  let onDisconnect: (error: unknown) => void = () => {};
  let ids = 0;
  let starts = 0;
  const relay = new Relay({
    connectHost: () => {
      starts += 1;
      return {
        post: (message: Record<string, unknown>) => toHost.push(message),
        onMessage: (listener: (message: unknown) => void) => (onMessage = listener),
        onDisconnect: (listener: (error: unknown) => void) => (onDisconnect = listener),
      };
    },
    now: () => 1_700_000_000_000,
    newId: () => `relay-${++ids}`,
    ...(replyWindow ? { replyWindow } : {}),
  });
  const page = (origin = ORIGIN) => {
    const got: Record<string, unknown>[] = [];
    return { got, handle: relay.addPage(origin, (message: Record<string, unknown>) => got.push(message)) };
  };
  return { relay, toHost, page, host: (message: unknown) => onMessage(message), drop: (error: unknown) => onDisconnect(error), starts: () => starts };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

test("a request is stamped as the service worker stamps it, and its reply goes back under the page's id", async () => {
  const h = harness();
  const a = h.page();
  assert.deepEqual(h.toHost[0], { v: 1, id: "relay-1", method: "pages", params: { origins: [ORIGIN] } });
  const answered = h.relay.fromPage(a.handle, JSON.stringify({ id: "page-1", method: "getResolution", params: { id: "r1" } }));
  const sent = h.toHost[1];
  assert.deepEqual(sent, { v: 1, id: "relay-2", origin: ORIGIN, method: "getResolution", params: { id: "r1" }, expiresAt: 1_700_000_015 });
  h.host({ v: 1, id: "relay-2", result: { resolution: { id: "r1", state: "done" } } });
  await answered;
  assert.deepEqual(a.got, [{ v: 1, id: "page-1", result: { resolution: { id: "r1", state: "done" } } }]);

  // One that waits for the person expires in ten minutes.
  void h.relay.fromPage(a.handle, JSON.stringify({ id: "page-2", method: "resolveMessage", params: {} }));
  assert.equal(h.toHost[2].expiresAt, 1_700_000_600);
  assert.equal(h.relay.pending().length, 1);
  // The error is passed whole, so a requirement reaches the page.
  h.host({ v: 1, id: h.toHost[2].id, error: { code: "unsupported", message: "No memo here.", requirement: "chain" } });
  await tick();
  assert.deepEqual(a.got[1], { v: 1, id: "page-2", error: { code: "unsupported", message: "No memo here.", requirement: "chain" } });
  assert.equal(h.relay.pending().length, 0);
});

test("what the service worker refuses, the relay refuses", async () => {
  const h = harness();
  const a = h.page();
  await h.relay.fromPage(a.handle, "not json");
  await h.relay.fromPage(a.handle, JSON.stringify({ id: "x1", method: "pages" }));
  await h.relay.fromPage(a.handle, JSON.stringify({ id: "has space", method: "status" }));
  assert.deepEqual(a.got[0], { v: 1, error: { code: "bad_request", message: "Missing request id." } });
  assert.deepEqual(a.got[1], { v: 1, id: "x1", error: { code: "bad_request", message: "Unsupported method." } });
  assert.equal((a.got[2].error as { code: string }).code, "bad_request");
  assert.equal(h.toHost.length, 1, "nothing but pages reached the host");
});

test("a push reaches only the pages of its origin, and the pages message follows who is connected", () => {
  const h = harness();
  const a = h.page();
  const b = h.page();
  const other = h.page("http://localhost:9000");
  // A second page of an origin already listed tells the app nothing.
  assert.equal(h.toHost.filter((m) => m.method === "pages").length, 2);
  assert.deepEqual((h.toHost.at(-1)!.params as { origins: string[] }).origins, ["http://127.0.0.1:8124", "http://localhost:9000"]);
  const push = { v: 1, method: "resolutionChanged", origin: ORIGIN, params: { resolution: { id: "r", state: "sending" } } };
  h.host(push);
  h.host({ v: 1, method: "somethingElse", origin: ORIGIN });
  assert.deepEqual(a.got, [{ push }]);
  assert.deepEqual(b.got, [{ push }]);
  assert.deepEqual(other.got, []);
  h.relay.removePage(other.handle);
  assert.deepEqual(h.toHost.at(-1)!.params, { origins: [ORIGIN] });
});

test("a page that goes away has what it waited on cancelled", async () => {
  const h = harness();
  const a = h.page();
  void h.relay.fromPage(a.handle, JSON.stringify({ id: "p1", method: "resolveMessage", params: {} }));
  void h.relay.fromPage(a.handle, JSON.stringify({ id: "p2", method: "getResolution", params: { id: "r" } }));
  const waitingId = h.toHost[1].id;
  h.relay.removePage(a.handle);
  const cancels = h.toHost.filter((m) => m.method === "cancel");
  // Only the request that waits for the person: the other is answered at once anyway.
  assert.deepEqual(cancels.map((m) => m.params), [{ id: waitingId }]);
  assert.equal(cancels[0].origin, undefined);
  assert.deepEqual(h.toHost.at(-1)?.method, "cancel");
  assert.equal(h.relay.pending().length, 0);
  // The quick one is answered after all, to a page that is no longer there.
  h.host({ v: 1, id: h.toHost[2].id, result: {} });
  await tick();
  assert.deepEqual(a.got, []);
});

test("when the host goes, every page waiting is told, and the next request starts it again", async () => {
  const h = harness();
  const a = h.page();
  const answered = h.relay.fromPage(a.handle, JSON.stringify({ id: "p1", method: "status" }));
  h.drop({ code: "app_not_running", message: "The native host exited (code 0)." });
  await answered;
  assert.deepEqual(a.got[0], { v: 1, id: "p1", error: { code: "app_not_running", message: "The native host exited (code 0)." } });
  assert.equal(h.starts(), 1);
  void h.relay.fromPage(a.handle, JSON.stringify({ id: "p2", method: "status" }));
  assert.equal(h.starts(), 2);
  // A new host hears which pages are open before anything else.
  assert.deepEqual(h.toHost.slice(-2).map((m) => m.method), ["pages", "status"]);
  h.host({ v: 1, id: h.toHost.at(-1)!.id, result: { ready: true } });
  await tick();
  assert.deepEqual(a.got[1], { v: 1, id: "p2", result: { ready: true } });
});

test("a request the app never answers expires, as in the service worker", async () => {
  const h = harness(() => 20);
  const a = h.page();
  await h.relay.fromPage(a.handle, JSON.stringify({ id: "p1", method: "status" }));
  assert.deepEqual(a.got[0], { v: 1, id: "p1", error: { code: "expired", message: "The desktop app did not answer in time." } });
});

/* End to end: a WebSocket client, the relay, and the fake app as its host. */

interface Client {
  send(message: unknown): void;
  next(match: (message: Record<string, unknown>) => boolean): Promise<Record<string, unknown>>;
  close(): void;
}

function wsConnect(port: number, origin: string): Promise<{ status: string; client: Client | null }> {
  return new Promise((resolve, reject) => {
    const socket: Socket = connectTcp(port, "127.0.0.1");
    const key = Buffer.from(Keypair.generate().publicKey.toBytes().subarray(0, 16)).toString("base64");
    let head = Buffer.alloc(0);
    let upgraded = false;
    const reader = new WsReader({ requireMask: false });
    const inbox: Record<string, unknown>[] = [];
    const waiters: { match: (m: Record<string, unknown>) => boolean; resolve: (m: Record<string, unknown>) => void }[] = [];
    const deliver = () => {
      for (const waiter of [...waiters]) {
        const index = inbox.findIndex(waiter.match);
        if (index < 0) continue;
        waiters.splice(waiters.indexOf(waiter), 1);
        waiter.resolve(inbox.splice(index, 1)[0]);
      }
    };
    const client: Client = {
      send: (message) => socket.write(encodeWsFrame(OPCODE.text, JSON.stringify(message), Buffer.from([7, 1, 7, 1]))),
      next: (match) =>
        new Promise((done, fail) => {
          waiters.push({ match, resolve: done });
          setTimeout(() => fail(new Error("no such message in 10 s")), 10_000).unref();
          deliver();
        }),
      close: () => socket.destroy(),
    };
    socket.on("error", reject);
    socket.on("data", (chunk: Buffer) => {
      if (!upgraded) {
        head = Buffer.concat([head, chunk]);
        const end = head.indexOf("\r\n\r\n");
        if (end < 0) return;
        const status = head.subarray(0, head.indexOf("\r\n")).toString();
        if (!status.includes(" 101 ")) {
          resolve({ status, client: null });
          socket.destroy();
          return;
        }
        assert.ok(head.toString().includes(`Sec-WebSocket-Accept: ${acceptKey(key)}`));
        upgraded = true;
        resolve({ status, client });
        chunk = head.subarray(end + 4);
      }
      for (const frame of reader.push(chunk)) inbox.push(JSON.parse(frame.payload.toString()));
      deliver();
    });
    socket.write(
      `GET / HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: ${key}\r\nSec-WebSocket-Version: 13\r\nOrigin: ${origin}\r\n\r\n`,
    );
  });
}

test("end to end: a page through the relay to a host, with pushes back", async () => {
  const lines: string[] = [];
  const relay = await startRelay({
    port: 0,
    hostPath: FAKE_APP,
    env: { ...process.env, FAKE_DELAY_MS: "50" },
    log: (direction: string, origin: string) => lines.push(`${direction} ${origin}`),
  });
  try {
    const refused = await wsConnect(relay.port, "https://app.example");
    assert.match(refused.status, / 403 /);

    const { client } = await wsConnect(relay.port, ORIGIN);
    assert.ok(client);
    client.send({ id: "c1", method: "connect", params: { silent: true } });
    assert.deepEqual((await client.next((m) => m.id === "c1")).result, { accounts: [] });
    client.send({ id: "c2", method: "connect", params: {} });
    const accounts = ((await client.next((m) => m.id === "c2")).result as { accounts: { publicKey: string }[] }).accounts;
    const account = Keypair.fromSeed(new Uint8Array(32).fill(1)).publicKey;
    assert.equal(accounts[0].publicKey, Buffer.from(account.toBytes()).toString("base64"));

    // A note on the account acted as is refused.
    const wire = Buffer.from(legacy(account, [memo("relay test", [account])])).toString("base64");
    client.send({
      id: "r0",
      method: "resolve",
      params: { id: "note-on-account", account: account.toBase58(), chain: "solana:devnet", transactions: [wire], accounts: { [account.toBase58()]: { kind: "feePayer", replaceable: true } } },
    });
    assert.equal(((await client.next((m) => m.id === "r0")).error as { code: string }).code, "bad_notes");

    client.send({ id: "r1", method: "resolve", params: { id: "memo-1", account: account.toBase58(), chain: "solana:devnet", transactions: [wire] } });
    const first = (await client.next((m) => m.id === "r1")).result as { resolution: { state: string } };
    assert.equal(first.resolution.state, "awaiting-approval");
    const done = await client.next((m) => (m.push as { params?: { resolution?: { state?: string } } })?.params?.resolution?.state === "done");
    const push = done.push as { origin: string; method: string; params: { resolution: { ran: unknown[] } } };
    assert.equal(push.origin, ORIGIN);
    assert.equal(push.method, "resolutionChanged");
    assert.equal(push.params.resolution.ran.length, 1);
    client.close();
  } finally {
    await relay.close();
  }
  // Every frame was logged, both ways.
  for (const direction of ["page>", ">host", "host>", ">page", "refused"]) assert.ok(lines.some((line) => line.startsWith(direction)), direction);
});
