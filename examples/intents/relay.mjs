// A relay between the test page's dev transport and the desktop app, for
// end-to-end runs with no browser extension. It does what the extension's
// service worker does (extension/src/background.ts): it starts the app's
// native messaging host, stamps each page request with `v`, an id of its own,
// the page's origin and `expiresAt`, matches replies back to the page's id,
// passes pushes to the pages of their origin, reports the origins with a page
// open (`pages`), and cancels what a page that went away was waiting on.
//
//   node relay.mjs <port> [<bridge-host>] [--quiet]
//
// The host is `<bridge-host>` or BRIDGE_HOST. A path ending in .mjs or .js is
// run with this Node, which is how the tests start a fake app. The
// environment is passed through, so USERNAME, LOCALAPPDATA and
// AIRLOCK_DATA_DIR choose which app the host reaches.
//
// It listens on 127.0.0.1 only and takes a WebSocket only from a loopback
// origin. Plain Node 22: the WebSocket server is written here, and takes
// unfragmented frames, which is all a browser sends for these messages.

import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { resolve as resolvePath } from "node:path";
import { fileURLToPath } from "node:url";

/* What the extension allows, as extension/src/protocol.ts states it. The
 * tests hold the two together. */

export const METHODS = ["status", "connect", "disconnect", "signTransaction", "signAndSendTransaction", "signAndSendAllTransactions", "signMessage", "signIn"];
export const INTENT_METHODS = ["getSigningProfile", "resolve", "getResolution", "provideSignatures", "cancelResolution", "resolveMessage"];
export const PUSH_METHODS = ["accountsChanged", "resolutionChanged"];
export const REQUEST_TTL_MS = 10 * 60 * 1000;
export const QUICK_TTL_MS = 15 * 1000;
export const MAX_TRANSACTIONS = 16;
const SEND_MS = MAX_TRANSACTIONS * 150 * 1000;
const REPLY_GRACE_MS = 5 * 1000;
export const MAX_PENDING_PER_ORIGIN = 8;
export const MAX_PENDING = 32;
export const RATE_LIMIT = 30;
export const RATE_WINDOW_MS = 10 * 1000;
export const MAX_REQUEST_BYTES = 256 * 1024;
const ANSWERED_AT_ONCE = ["getSigningProfile", "resolve", "getResolution", "provideSignatures", "cancelResolution"];

export function isBridgeMethod(value) {
  return METHODS.includes(value) || INTENT_METHODS.includes(value);
}

export function isRequestId(id) {
  return typeof id === "string" && /^[A-Za-z0-9-]{1,80}$/.test(id);
}

function isSilentConnect(method, params) {
  return method === "connect" && params?.silent === true;
}

export function waitsForUser(method, params) {
  return method !== "status" && method !== "disconnect" && !ANSWERED_AT_ONCE.includes(method) && !isSilentConnect(method, params);
}

export function replyWindowMs(method, params) {
  if (!waitsForUser(method, params)) return QUICK_TTL_MS;
  const sends = method === "signAndSendTransaction" || method === "signAndSendAllTransactions";
  return REQUEST_TTL_MS + (sends ? SEND_MS : 0) + REPLY_GRACE_MS;
}

function jsonBytes(value) {
  try {
    return Buffer.byteLength(JSON.stringify(value) ?? "");
  } catch {
    return Infinity;
  }
}

/* Native messaging: a 32-bit little-endian length, then the JSON. */

/** The most a frame may carry, as bridge_protocol::frame reads. */
export const MAX_FRAME = 1024 * 1024;

export function encodeFrame(message) {
  const body = Buffer.from(JSON.stringify(message), "utf8");
  if (body.length > MAX_FRAME) throw new Error(`A frame of ${body.length} bytes is more than the host reads.`);
  const head = Buffer.alloc(4);
  head.writeUInt32LE(body.length, 0);
  return Buffer.concat([head, body]);
}

/** Reads frames out of a stream that may split them, or join several, at any byte. */
export class FrameReader {
  #buffer = Buffer.alloc(0);

  /** The messages that `chunk` completes. Throws on a frame too large or not JSON. */
  push(chunk) {
    this.#buffer = this.#buffer.length ? Buffer.concat([this.#buffer, chunk]) : Buffer.from(chunk);
    const out = [];
    while (this.#buffer.length >= 4) {
      const length = this.#buffer.readUInt32LE(0);
      if (length > MAX_FRAME) throw new Error(`The host sent a frame of ${length} bytes.`);
      if (this.#buffer.length < 4 + length) break;
      const body = this.#buffer.subarray(4, 4 + length).toString("utf8");
      this.#buffer = this.#buffer.subarray(4 + length);
      out.push(JSON.parse(body));
    }
    return out;
  }
}

/* WebSocket (RFC 6455): the handshake, and unfragmented frames. */

const WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
export const OPCODE = { text: 0x1, binary: 0x2, close: 0x8, ping: 0x9, pong: 0xa };
/** The most one message from a page may take: more than any request the extension lets through. */
export const MAX_WS_MESSAGE = 1024 * 1024;

export function acceptKey(key) {
  return createHash("sha1").update(key + WS_GUID).digest("base64");
}

/** One frame, final. A client masks what it sends; a server does not. */
export function encodeWsFrame(opcode, payload, mask) {
  const body = Buffer.isBuffer(payload) ? payload : Buffer.from(payload ?? "", "utf8");
  const head = [0x80 | opcode];
  const masked = mask ? 0x80 : 0;
  if (body.length < 126) head.push(masked | body.length);
  else if (body.length < 65536) head.push(masked | 126, body.length >> 8, body.length & 0xff);
  else {
    head.push(masked | 127, 0, 0, 0, 0);
    head.push((body.length >>> 24) & 0xff, (body.length >>> 16) & 0xff, (body.length >>> 8) & 0xff, body.length & 0xff);
  }
  if (!mask) return Buffer.concat([Buffer.from(head), body]);
  const out = Buffer.from(body);
  for (let i = 0; i < out.length; i += 1) out[i] ^= mask[i % 4];
  return Buffer.concat([Buffer.from(head), Buffer.from(mask), out]);
}

/** Reads frames out of a socket. With `requireMask`, as a server must, an
 * unmasked frame is refused. Throws on what this relay does not take. */
export class WsReader {
  #buffer = Buffer.alloc(0);
  #requireMask;

  constructor({ requireMask = true } = {}) {
    this.#requireMask = requireMask;
  }

  push(chunk) {
    this.#buffer = this.#buffer.length ? Buffer.concat([this.#buffer, chunk]) : Buffer.from(chunk);
    const out = [];
    for (;;) {
      const b = this.#buffer;
      if (b.length < 2) break;
      const fin = (b[0] & 0x80) !== 0;
      const opcode = b[0] & 0x0f;
      const masked = (b[1] & 0x80) !== 0;
      let length = b[1] & 0x7f;
      let at = 2;
      if (length === 126) {
        if (b.length < 4) break;
        length = b.readUInt16BE(2);
        at = 4;
      } else if (length === 127) {
        if (b.length < 10) break;
        const high = b.readUInt32BE(2);
        if (high !== 0) throw new Error("A WebSocket frame larger than 4 GiB.");
        length = b.readUInt32BE(6);
        at = 10;
      }
      if (!fin || opcode === 0) throw new Error("A fragmented WebSocket message: this relay takes whole messages only.");
      if (length > MAX_WS_MESSAGE) throw new Error(`A WebSocket message of ${length} bytes.`);
      if (this.#requireMask && !masked) throw new Error("An unmasked frame from a client.");
      const maskAt = at;
      if (masked) at += 4;
      if (b.length < at + length) break;
      const payload = Buffer.from(b.subarray(at, at + length));
      if (masked) for (let i = 0; i < payload.length; i += 1) payload[i] ^= b[maskAt + (i % 4)];
      out.push({ opcode, payload });
      this.#buffer = b.subarray(at + length);
    }
    return out;
  }
}

const LOOPBACK_HOSTS = new Set(["127.0.0.1", "localhost", "[::1]"]);

/** Whether `origin` is a page served from this machine. */
export function isLoopbackOrigin(origin) {
  if (typeof origin !== "string") return false;
  try {
    const url = new URL(origin);
    return (url.protocol === "http:" || url.protocol === "https:") && LOOPBACK_HOSTS.has(url.hostname) && url.origin === origin;
  } catch {
    return false;
  }
}

/** Whether a Host header names this machine, so a page on another name that
 * resolves here (DNS rebinding) is turned away. */
export function isLoopbackHost(host) {
  if (typeof host !== "string") return false;
  try {
    return LOOPBACK_HOSTS.has(new URL(`http://${host}`).hostname);
  } catch {
    return false;
  }
}

/* The bookkeeping of background.ts, with pages in place of its ports. */

/**
 * `connectHost()` starts the host and returns a port:
 *   { post(message), onMessage(listener), onDisconnect(listener(error)) }
 * where `error` is `{ code, message }`. `log(direction, origin, message)` is
 * told every frame: "page>" from a page, ">page" to it, ">host" and "host>".
 */
export class Relay {
  #pages = new Map();
  #waiters = new Map();
  #pending = [];
  #askers = new Map();
  #asked = new Map();
  #rateWindowStart = 0;
  #reported = null;
  #port = null;
  #connectHost;
  #log;
  #now;
  #newId;
  #replyWindow;

  constructor({ connectHost, log = () => {}, now = Date.now, newId = randomUUID, replyWindow = replyWindowMs }) {
    this.#connectHost = connectHost;
    this.#log = log;
    this.#now = now;
    this.#newId = newId;
    this.#replyWindow = replyWindow;
  }

  /** A page connected from `origin`. `deliver(message)` sends it a message. */
  addPage(origin, deliver) {
    const page = { origin, deliver };
    this.#pages.set(page, origin);
    // The extension reports pages only once the host runs. Here the host is
    // started for the first page, so the app shows the site as connected.
    try {
      this.#connect();
    } catch {
      // The page's first request says why.
    }
    this.#reportPages();
    return page;
  }

  /** Starts the host before any page connects. */
  start() {
    this.#connect();
  }

  removePage(page) {
    if (!this.#pages.delete(page)) return;
    this.#reportPages();
    this.#abandon(page);
  }

  /** The origins with a page open, as `pages` reports them. */
  origins() {
    return [...new Set(this.#pages.values())].sort();
  }

  /** Requests that wait for the person, by the relay's own id. */
  pending() {
    return this.#pending.map((item) => ({ ...item }));
  }

  /** A text message from a page. Its answer goes back through the page's `deliver`. */
  async fromPage(page, text) {
    let body;
    try {
      body = JSON.parse(text);
    } catch {
      body = null;
    }
    this.#log("page>", page.origin, body ?? text);
    const response = await this.#handle(page, body);
    if (!this.#pages.has(page)) return;
    this.#send(page, response);
  }

  async #handle(page, body) {
    const record = body && typeof body === "object" ? body : {};
    const pageId = record.id;
    if (!isRequestId(pageId)) return { v: 1, error: { code: "bad_request", message: "Missing request id." } };
    const refuse = (code, message) => ({ v: 1, id: pageId, error: { code, message } });
    if (typeof record.method !== "string" || !isBridgeMethod(record.method)) return refuse("bad_request", "Unsupported method.");
    if (!this.#withinRate(page.origin)) return refuse("too_many_requests", "This site is asking too often.");
    const waits = waitsForUser(record.method, record.params);
    if (waits && !this.#hasRoom(page.origin)) return refuse("too_many_requests", "Too many requests are waiting in Airlock.");
    // An id of the relay's own: another page's reply must never reach this one.
    const request = {
      v: 1,
      id: this.#newId(),
      origin: page.origin,
      method: record.method,
      params: record.params,
      expiresAt: Math.floor((this.#now() + (waits ? REQUEST_TTL_MS : QUICK_TTL_MS)) / 1000),
    };
    if (jsonBytes(request) > MAX_REQUEST_BYTES) return refuse("bad_request", "The request is too large.");
    return { ...(await this.#sendNative(request, waits ? page : undefined)), id: pageId };
  }

  #withinRate(origin) {
    const now = this.#now();
    if (now - this.#rateWindowStart >= RATE_WINDOW_MS) {
      this.#rateWindowStart = now;
      this.#asked.clear();
    }
    const count = (this.#asked.get(origin) ?? 0) + 1;
    this.#asked.set(origin, count);
    return count <= RATE_LIMIT;
  }

  #hasRoom(origin) {
    const fromSite = this.#pending.filter((item) => item.origin === origin).length;
    return this.#pending.length < MAX_PENDING && fromSite < MAX_PENDING_PER_ORIGIN;
  }

  /** The page is gone, so nobody waits on what it asked: the app drops it. */
  #abandon(page) {
    for (const [id, asker] of [...this.#askers]) {
      if (asker !== page) continue;
      this.#waiters.get(id)?.({ v: 1, error: { code: "expired", message: "The page closed." } });
      this.#post({ v: 1, id: this.#newId(), method: "cancel", params: { id } });
    }
  }

  #sendNative(request, asker) {
    const track = asker !== undefined;
    return new Promise((resolve) => {
      let settled = false;
      const finish = (message) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        this.#waiters.delete(request.id);
        if (track) {
          this.#askers.delete(request.id);
          const index = this.#pending.findIndex((item) => item.id === request.id);
          if (index >= 0) this.#pending.splice(index, 1);
        }
        resolve({ ...message, id: request.id });
      };
      const timer = setTimeout(
        () => finish({ v: 1, error: { code: "expired", message: "The desktop app did not answer in time." } }),
        this.#replyWindow(request.method, request.params),
      );
      this.#waiters.set(request.id, finish);
      if (track) {
        this.#askers.set(request.id, asker);
        this.#pending.push({ id: request.id, origin: request.origin, method: request.method });
      }
      const error = this.#post(request);
      if (error) finish({ v: 1, error });
    });
  }

  /** Posts to the host, starting it if need be. Returns the error when it cannot. */
  #post(message) {
    try {
      const port = this.#connect();
      this.#log(">host", message.origin ?? "", message);
      port.post(message);
      return null;
    } catch (err) {
      return { code: "host_not_found", message: err instanceof Error ? err.message : "The native host could not be started." };
    }
  }

  #connect() {
    if (this.#port) return this.#port;
    const port = this.#connectHost();
    this.#port = port;
    this.#reported = null;
    port.onMessage((message) => this.#fromHost(message));
    port.onDisconnect((error) => {
      if (this.#port !== port) return;
      this.#port = null;
      for (const waiter of [...this.#waiters.values()]) waiter({ v: 1, error });
    });
    this.#postPages(port);
    return port;
  }

  #fromHost(message) {
    this.#log("host>", message?.origin ?? "", message);
    if (PUSH_METHODS.includes(message?.method)) {
      // Only that site's pages: another site must not learn its accounts, or
      // what became of its requests.
      for (const [page, origin] of this.#pages) {
        if (origin === message.origin) this.#send(page, { push: message });
      }
      return;
    }
    if (message?.id) this.#waiters.get(message.id)?.(message);
  }

  #send(page, message) {
    this.#log(">page", page.origin, message);
    try {
      page.deliver(message);
    } catch {
      this.removePage(page);
    }
  }

  #reportPages() {
    if (this.#port) this.#postPages(this.#port);
  }

  #postPages(port) {
    const origins = this.origins();
    const list = JSON.stringify(origins);
    if (list === this.#reported || list.length > MAX_REQUEST_BYTES) return;
    this.#reported = list;
    const message = { v: 1, id: this.#newId(), method: "pages", params: { origins } };
    this.#log(">host", "", message);
    try {
      port.post(message);
    } catch {
      // The host went away. It hears this again when it next starts.
    }
  }
}

/* The host process. */

/** Starts `command` with `args` and speaks native messaging to it. */
export function spawnHost(command, args = [], env = process.env, onStderr = () => {}) {
  const messageListeners = [];
  const disconnectListeners = [];
  const reader = new FrameReader();
  const child = spawn(command, args, { env, stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
  let gone = false;
  let stderrTail = "";
  const disconnect = (error) => {
    if (gone) return;
    gone = true;
    for (const listener of disconnectListeners) listener(error);
  };
  child.stdout.on("data", (chunk) => {
    let messages;
    try {
      messages = reader.push(chunk);
    } catch (err) {
      child.kill();
      disconnect({ code: "app_not_running", message: `The host sent what is not native messaging: ${err.message}` });
      return;
    }
    for (const message of messages) for (const listener of messageListeners) listener(message);
  });
  child.stderr.on("data", (chunk) => {
    const text = chunk.toString("utf8");
    stderrTail = (stderrTail + text).slice(-400);
    onStderr(text);
  });
  child.on("error", (err) => {
    disconnect({
      code: err.code === "ENOENT" ? "host_not_found" : "app_not_running",
      message: err.code === "ENOENT" ? `The host ${command} was not found.` : err.message,
    });
  });
  child.on("exit", (code, signal) => {
    const tail = stderrTail.trim().split("\n").pop();
    disconnect({ code: "app_not_running", message: `The native host exited (${signal ?? `code ${code}`})${tail ? `: ${tail}` : "."}` });
  });
  child.stdin.on("error", () => {
    // The host went away; its exit says so.
  });
  return {
    child,
    post(message) {
      if (gone) throw new Error("The native host is not running.");
      child.stdin.write(encodeFrame(message));
    },
    onMessage(listener) {
      messageListeners.push(listener);
    },
    onDisconnect(listener) {
      disconnectListeners.push(listener);
    },
    stop() {
      gone = true;
      child.stdin.end();
      child.kill();
    },
  };
}

/** How to start a host from its path: a script runs with this Node. */
export function hostCommand(path) {
  return /\.(mjs|cjs|js)$/i.test(path) ? [process.execPath, [path]] : [path, []];
}

/* The WebSocket server. */

/**
 * Listens on 127.0.0.1:`port` (0 for any) and relays each page to the host
 * at `hostPath`. Resolves once listening, with `{ port, relay, close() }`.
 */
export function startRelay({ port, hostPath, env = process.env, log = () => {}, onStderr = () => {} }) {
  const [command, args] = hostCommand(hostPath);
  const hosts = [];
  const relay = new Relay({
    connectHost: () => {
      const host = spawnHost(command, args, env, onStderr);
      hosts.push(host);
      return host;
    },
    log,
  });
  const sockets = new Set();
  const server = createServer((request, response) => {
    response.writeHead(426, { "content-type": "text/plain; charset=utf-8" });
    response.end("This is the Airlock intents relay. Open the test page with ?wallet=ws://127.0.0.1:<this port>.\n");
  });
  server.on("upgrade", (request, socket) => {
    const origin = request.headers.origin;
    const key = request.headers["sec-websocket-key"];
    const refuse = (status, text) => {
      log("refused", String(origin ?? ""), text);
      socket.end(`HTTP/1.1 ${status}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n${text}\n`);
    };
    if (String(request.headers.upgrade).toLowerCase() !== "websocket" || typeof key !== "string" || request.headers["sec-websocket-version"] !== "13") {
      refuse("400 Bad Request", "Not a WebSocket handshake.");
      return;
    }
    if (!isLoopbackHost(request.headers.host)) {
      refuse("403 Forbidden", "The relay answers only on this machine's own names.");
      return;
    }
    if (!isLoopbackOrigin(origin)) {
      refuse("403 Forbidden", `Only a page served from this machine may connect, not ${origin ?? "one with no origin"}.`);
      return;
    }
    socket.write(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${acceptKey(key)}\r\n\r\n`,
    );
    sockets.add(socket);
    const reader = new WsReader();
    const page = relay.addPage(origin, (message) => socket.write(encodeWsFrame(OPCODE.text, JSON.stringify(message))));
    let closed = false;
    const close = () => {
      if (closed) return;
      closed = true;
      sockets.delete(socket);
      relay.removePage(page);
    };
    socket.on("data", (chunk) => {
      let frames;
      try {
        frames = reader.push(chunk);
      } catch (err) {
        log("refused", origin, err.message);
        socket.end(encodeWsFrame(OPCODE.close, Buffer.from([0x03, 0xf0])));
        close();
        return;
      }
      for (const { opcode, payload } of frames) {
        if (opcode === OPCODE.text) void relay.fromPage(page, payload.toString("utf8"));
        else if (opcode === OPCODE.ping) socket.write(encodeWsFrame(OPCODE.pong, payload));
        else if (opcode === OPCODE.close) {
          socket.end(encodeWsFrame(OPCODE.close, payload.subarray(0, 2)));
          close();
        }
      }
    });
    socket.on("close", close);
    socket.on("error", close);
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", () => {
      resolve({
        port: server.address().port,
        relay,
        close: () =>
          new Promise((done) => {
            for (const socket of sockets) socket.destroy();
            for (const host of hosts) host.stop();
            server.close(() => done());
          }),
      });
    });
  });
}

/* The command line. */

function stamp() {
  return new Date().toISOString().slice(11, 23);
}

/** One line for a frame: the time, where it went, the origin, and the JSON. */
export function logLine(direction, origin, message) {
  const text = typeof message === "string" ? message : JSON.stringify(message);
  return `${stamp()}  ${direction.padEnd(7)} ${origin ? `${origin}  ` : ""}${text}`;
}

async function main() {
  const args = process.argv.slice(2);
  const quiet = args.includes("--quiet");
  const positional = args.filter((arg) => !arg.startsWith("--"));
  const port = Number(positional[0]);
  const hostPath = positional[1] ?? process.env.BRIDGE_HOST;
  if (!Number.isInteger(port) || port <= 0 || port > 65535 || !hostPath) {
    console.error("Usage: node relay.mjs <port> [<bridge-host>] [--quiet]\n  The host is the path given, or BRIDGE_HOST: target/debug/bridge-host.exe for a debug build.");
    process.exit(2);
  }
  const host = resolvePath(hostPath);
  const started = await startRelay({
    port,
    hostPath: host,
    log: quiet ? () => {} : (direction, origin, message) => console.log(logLine(direction, origin, message)),
    onStderr: quiet ? () => {} : (text) => process.stderr.write(text),
  }).catch((err) => {
    console.error(`The relay could not listen on 127.0.0.1:${port}: ${err.message}`);
    process.exit(1);
  });
  console.log(`Relay on ws://127.0.0.1:${started.port}, host ${host}. Open the page with ?wallet=ws://127.0.0.1:${started.port}`);
  const stop = async () => {
    await started.close();
    process.exit(0);
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
}

// Run only as `node relay.mjs`, not when imported (the tests bundle it).
if (process.argv[1] && resolvePath(process.argv[1]) === fileURLToPath(import.meta.url) && import.meta.url.endsWith("/relay.mjs")) {
  await main();
}
