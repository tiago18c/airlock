// Drives the desktop app through browser sign requests, without a browser or
// the extension, and screenshots the request modal. See "Driving the desktop
// app" in AGENTS.md.
//
//   cargo build -p airlock -p bridge-host
//   node scripts/drive-desktop.mjs <outdir> [scenario...]
//
// Windows only: the debugging port is WebView2's. Each scenario writes
// <outdir>/<scenario>.png and prints the modal's buttons. With no scenario
// named, every one runs. The app gets its own vault, WebView2 profile and
// bridge pipe under <outdir>, so an Airlock already running is left alone.
// DRIVE_PORT picks the debugging port (9555).
//
// For before and after screenshots, build `main`, run this into one folder,
// then build the branch and run it into another.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import crypto from "node:crypto";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = process.argv[2];
if (!out) {
  console.error("usage: node scripts/drive-desktop.mjs <outdir> [scenario...]");
  process.exit(2);
}
const port = Number(process.env.DRIVE_PORT ?? 9555);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---- keys and transactions ----

// A throwaway key, the same every run so screenshots match. Never fund it.
const seed = Buffer.alloc(32, 1);
const pkcs8 = Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), seed]);
const privateKey = crypto.createPrivateKey({ key: pkcs8, format: "der", type: "pkcs8" });
const payer = crypto.createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32);

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const b58decode = (s) => {
  let n = 0n;
  for (const c of s) n = n * 58n + BigInt(B58.indexOf(c));
  const bytes = [];
  while (n > 0n) { bytes.unshift(Number(n & 255n)); n >>= 8n; }
  for (const c of s) { if (c !== "1") break; bytes.unshift(0); }
  return Buffer.from(bytes);
};
const b58encode = (buf) => {
  let n = BigInt("0x" + (Buffer.from(buf).toString("hex") || "0"));
  let s = "";
  while (n > 0n) { s = B58[Number(n % 58n)] + s; n /= 58n; }
  for (const b of buf) { if (b !== 0) break; s = "1" + s; }
  return s;
};

const recipient = b58decode("4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T");
const systemProgram = Buffer.alloc(32);
// A program no IDL on the device describes.
const unknownProgram = Buffer.alloc(32, 7);

// A recent devnet blockhash, so the modal counts down. Offline, a zero one.
let blockhash = Buffer.alloc(32);
try {
  const res = await fetch("https://api.devnet.solana.com", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getLatestBlockhash", params: [{ commitment: "finalized" }] }),
  });
  blockhash = b58decode((await res.json()).result.value.blockhash);
} catch {
  console.log("no devnet blockhash, using a zero one");
}

const transferData = (lamports) => {
  const data = Buffer.alloc(12);
  data.writeUInt32LE(2, 0);
  data.writeBigUInt64LE(BigInt(lamports), 4);
  return data;
};

/** A length as the wire writes it: 7 bits a byte, low first. */
const shortVec = (n) => {
  const bytes = [];
  do {
    bytes.push((n & 0x7f) | (n > 0x7f ? 0x80 : 0));
    n >>= 7;
  } while (n > 0);
  return Buffer.from(bytes);
};

/** A legacy transaction paid and signed by `payer`, unsigned. `accounts`
 * follow the payer: writable ones, then `readonly` read-only ones at the end.
 * Each instruction is `[programIndex, accountIndexes, data]`. */
const legacy = (accounts, readonly, instructions) => {
  const keys = [payer, ...accounts];
  const compiled = instructions.map(([program, indexes, data]) =>
    Buffer.concat([Buffer.from([program, indexes.length, ...indexes]), shortVec(data.length), data]));
  const message = Buffer.concat([
    Buffer.from([1, 0, readonly, keys.length]), ...keys, blockhash,
    Buffer.from([instructions.length]), ...compiled,
  ]);
  return Buffer.concat([Buffer.from([1]), Buffer.alloc(64), message]);
};

/** A SOL transfer the app reads in words with the IDLs it ships. */
const transfer = (lamports) =>
  legacy([recipient, systemProgram], 1, [[2, [0, 1], transferData(lamports)]]);

/** A transfer and an instruction of a program with no IDL on the device. */
const unread = (lamports) =>
  legacy([recipient, systemProgram, unknownProgram], 2, [
    [2, [0, 1], transferData(lamports)],
    [3, [0], Buffer.from([1, 2, 3])],
  ]);

/** A Realms `InsertTransaction` that adds a token transfer, which the
 * governance's treasury signs if the proposal passes, to a devnet proposal.
 * The payer adds it as the proposal owner. */
const realmsInsert = () => {
  const key = (s) => b58decode(s);
  const data = Buffer.from(
    "09000100000000000100000006ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf5857eff00a9" +
    "040000000f3b53f07edb5d8db314d5bf2b85ef0c4b638456d6c8c8b7bb273e9ce4db6f2600019323c44b18fe" +
    "83e5279c6c93ef3cddc028a94b7981ce5c7c96d76d1d08d9afb2000037f663db6099732f62f62a024c412211" +
    "b8e1b38f453d51d1d8aac1115a64df8900015bc23274bb4904732133affcad237649e2ca951d63604c92ff8c" +
    "eae870bda45e01000a0000000c0010a5d4e800000006",
    "hex",
  );
  // payer, proposal, proposal transaction | governance, owner record, System, Rent, Realms
  return legacy([
    key("BYbRt5XhHoagQv7mnGdDsEGbVaC5eeM4oFQhr9P4KjYa"),
    key("6AFtFZvFKeBrRebJK8TRYxTaUwAaqcaMjuZSNzJ9XHz2"),
    key("UUqdy9qMnfEjgzMo5xsijZ2iZhLo66fnfkMyTUYKSFz"),
    key("BarM4bouviowiVdB71dHVLwwcknLPeJ7X6s3KtAu5myu"),
    systemProgram,
    key("SysvarRent111111111111111111111111111111111"),
    key("GovER5Lthms3bLBqWub97yVrMmEogzX7xNjdXpPPCVZw"),
  ], 5, [[7, [3, 1, 4, 0, 2, 0, 5, 6], data]]);
};

/** Publishing program-metadata's own IDL: allocate its `idl` metadata
 * account, initialize it, and make it immutable. */
const programMetadata = b58decode("ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S");
const metadata = () => {
  const account = b58decode("9zzVPVwZojagC4mBCMrTi17Zu9KvXKoTvqnBL1vxYDyM");
  const programData = b58decode("F8hZ6cQFCNZ3UdNkhUfjgxxRnUD8HiYPobpk2qRVD7aB");
  const seed = Buffer.alloc(16);
  seed.write("idl");
  return legacy([account, programMetadata, programData, systemProgram], 3, [
    [2, [1, 0, 2, 3, 4], Buffer.concat([Buffer.from([7]), seed])],
    // utf8, zlib, json, direct, and the IDL in a later write
    [2, [1, 0, 2, 3, 2], Buffer.concat([Buffer.from([1]), seed, Buffer.from([1, 2, 1, 0])])],
    [2, [1, 0, 2, 3], Buffer.from([4])],
  ]);
};

// ---- scenarios: the transactions of one signTransaction request each ----

const scenarios = {
  transfer: () => [transfer(5)],
  unread: () => [unread(9)],
  batch: () => [transfer(5), unread(9)],
  realms: () => [realmsInsert()],
  metadata: () => [metadata()],
};
const chosen = process.argv.slice(3);
for (const name of chosen) {
  if (!scenarios[name]) {
    console.error(`unknown scenario ${name}; one of: ${Object.keys(scenarios).join(", ")}`);
    process.exit(2);
  }
}

// ---- app ----

const exe = (name) => join(root, "target", "debug", `${name}.exe`);
for (const name of ["airlock", "bridge-host"]) {
  if (!existsSync(exe(name))) {
    console.error(`${exe(name)} is missing: cargo build -p airlock -p bridge-host`);
    process.exit(1);
  }
}
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const env = {
  ...process.env,
  // Set by `cargo run`. Without it dioxus takes the exe for a bundle and
  // looks for its assets, the IDL pack among them, beside it.
  CARGO_MANIFEST_DIR: join(root, "apps", "airlock"),
  AIRLOCK_DATA_DIR: join(out, "data"),
  LOCALAPPDATA: join(out, "local"),
  USERNAME: `drive-${port}`,
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
};
const app = spawn(exe("airlock"), [], { env, stdio: ["ignore", "ignore", "pipe"] });
app.stderr.on("data", (d) => writeFileSync(join(out, "app.log"), d, { flag: "a" }));
let host;
const stop = (code) => {
  host?.stdin.end();
  host?.kill();
  app.kill();
  process.exit(code);
};

let target;
for (let i = 0; i < 120 && !target; i++) {
  await sleep(500);
  try {
    target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((t) => t.type === "page");
  } catch {}
}
if (!target) {
  console.error(`no page on port ${port}`);
  stop(1);
}
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let nextId = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
};
const send = (method, params = {}) => new Promise((r) => {
  const id = ++nextId;
  pending.set(id, r);
  ws.send(JSON.stringify({ id, method, params }));
});
const evaluate = async (expression) => {
  const res = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (res.result?.exceptionDetails) console.log("eval error:", JSON.stringify(res.result.exceptionDetails).slice(0, 400));
  return res.result?.result?.value;
};
await send("Page.enable");
await send("Runtime.enable");
await send("Emulation.setDeviceMetricsOverride", { width: 1200, height: 900, deviceScaleFactor: 1, mobile: false });
for (let i = 0; i < 60; i++) {
  await sleep(500);
  if (await evaluate(`!!document.querySelector('button')`)) break;
}
// A debug build started on its own does not load the stylesheets.
const css = ["tokens.css", "main.css"]
  .map((f) => readFileSync(join(root, "crates", "app-ui-kit", "assets", f), "utf8"))
  .join("\n");
await evaluate(`(() => { const s = document.createElement('style'); s.textContent = ${JSON.stringify(css)}; document.head.appendChild(s); return true; })()`);

// Run `body` in the page, with these helpers in scope.
const helpers = `
  const w = (ms) => new Promise((r) => setTimeout(r, ms));
  const visible = (el) => el && el.offsetParent !== null;
  const button = (text, root = document) => [...root.querySelectorAll('button')].find((b) => b.innerText.trim() === text && visible(b));
  const field = (text, root = document) => [...root.querySelectorAll('label')].find((l) => l.innerText.trim().startsWith(text));
  const modal = () => document.querySelector('.modal');
  const type = (el, value) => { const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value); el.dispatchEvent(new Event('input', { bubbles: true })); };
  const until = async (f, ms = 15000) => { for (let t = 0; t < ms; t += 200) { const v = f(); if (v) return v; await w(200); } return null; };
  const buttons = () => [...document.querySelectorAll('button')].filter(visible).map((b) => b.innerText.trim()).join(' | ');
`;
const run = (body) => evaluate(`(async () => { ${helpers} ${body} })()`);

const wallet = await run(`
  for (const p of document.querySelectorAll('input[type=password]')) type(p, 'correct horse battery');
  await w(200);
  button('Create vault')?.click();
  if (!await until(() => button('Add wallet'))) return 'no Add wallet: ' + buttons();
  button('Add wallet').click();
  await w(500);
  button('Private key', modal())?.click();
  await w(300);
  type(field('Label', modal()).querySelector('input'), 'Main');
  type(field('Private key', modal()).querySelector('textarea'), '${seed.toString("hex")}');
  await w(300);
  button('Save wallet', modal()).click();
  await w(1500);
  return document.body.innerText.includes('${b58encode(payer).slice(0, 4)}') ? 'ok' : 'not seen: ' + buttons();
`);
if (wallet !== "ok") {
  console.error(`wallet: ${wallet}`);
  stop(1);
}

// ---- bridge-host ----

host = spawn(exe("bridge-host"), [], { env, stdio: ["pipe", "pipe", "ignore"] });
let buffer = Buffer.alloc(0);
const replies = [];
host.stdout.on("data", (d) => {
  buffer = Buffer.concat([buffer, d]);
  while (buffer.length >= 4) {
    const len = buffer.readUInt32LE(0);
    if (buffer.length < 4 + len) break;
    replies.push(JSON.parse(buffer.subarray(4, 4 + len).toString("utf8")));
    buffer = buffer.subarray(4 + len);
  }
});
// A native messaging frame: a 32-bit little-endian length, then the JSON.
const frame = (obj) => {
  const body = Buffer.from(JSON.stringify(obj));
  const head = Buffer.alloc(4);
  head.writeUInt32LE(body.length);
  host.stdin.write(Buffer.concat([head, body]));
};
const replyTo = async (id, ms = 20000) => {
  for (let t = 0; t < ms; t += 100) {
    const reply = replies.find((r) => r.id === id);
    if (reply) return reply;
    await sleep(100);
  }
  return null;
};
frame({ v: 1, id: "status-1", method: "status" });
if ((await replyTo("status-1"))?.result?.role !== "desktop") {
  console.error("bridge-host does not reach the app");
  stop(1);
}

// ---- requests ----

/** Screenshot the request modal, scrolled to its top. */
const shotModal = async (path) => {
  const r = await evaluate(`(() => { const m = document.querySelector('.modal.request'); m.scrollTop = 0; const b = m.getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height }; })()`);
  const res = await send("Page.captureScreenshot", {
    format: "png",
    clip: { x: r.x - 8, y: r.y - 8, width: r.w + 16, height: r.h + 16, scale: 1 },
  });
  writeFileSync(path, Buffer.from(res.result.data, "base64"));
};

let n = 0;
for (const name of chosen.length ? chosen : Object.keys(scenarios)) {
  const id = `${(++n).toString(16).padStart(4, "a")}-${name}`;
  frame({
    v: 1, id, origin: "https://drive.example", method: "signTransaction",
    expiresAt: Math.floor(Date.now() / 1000) + 600,
    params: {
      transactions: scenarios[name]().map((wire) => ({
        account: b58encode(payer),
        transaction: wire.toString("base64"),
        chain: "solana:devnet",
      })),
    },
  });
  // Ready once Review is enabled and no IDL is still being looked up on chain.
  const state = await run(`
    if (!await until(() => document.querySelector('.modal.request'))) return 'no modal';
    await until(() => { const b = button('Review'); return b && !b.disabled; }, 30000);
    await until(() => !document.body.innerText.includes('Looking for the IDLs'), 30000);
    await w(1500);
    return [...document.querySelectorAll('.modal.request .btn-row button')].filter(visible)
      .map((b) => b.innerText.trim() + (b.className ? ' [' + b.className + ']' : '') + (b.disabled ? ' (disabled)' : ''))
      .join(', ');
  `);
  console.log(`${name}: ${state}`);
  if (state !== "no modal") await shotModal(join(out, `${name}.png`));
  await run(`button('Reject')?.click(); await w(800); return 1;`);
  const reply = await replyTo(id, 5000);
  if (reply?.error?.code !== "rejected") console.log(`${name}: reply ${JSON.stringify(reply)}`);
}
stop(0);
