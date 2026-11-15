// Helpers for driving the desktop app without a browser or the extension, on
// Windows: start a debug build on its own data folder, drive its page over
// the Chrome DevTools Protocol, and talk to it through `bridge-host` as the
// extension would. See "Driving the desktop app" in AGENTS.md.
//
// `scripts/drive-intents.mjs` uses these. Each helper takes what it needs and
// returns what it made, so a script can start the app, add a wallet, connect
// a site, send frames and stop everything in whatever order it needs.
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import crypto from "node:crypto";

export const root = join(dirname(fileURLToPath(import.meta.url)), "..");
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---- base58, keys ----

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
export const b58decode = (s) => {
  let n = 0n;
  for (const c of s) n = n * 58n + BigInt(B58.indexOf(c));
  const bytes = [];
  while (n > 0n) { bytes.unshift(Number(n & 255n)); n >>= 8n; }
  for (const c of s) { if (c !== "1") break; bytes.unshift(0); }
  return Buffer.from(bytes);
};
export const b58encode = (buf) => {
  let n = BigInt("0x" + (Buffer.from(buf).toString("hex") || "0"));
  let s = "";
  while (n > 0n) { s = B58[Number(n % 58n)] + s; n /= 58n; }
  for (const b of buf) { if (b !== 0) break; s = "1" + s; }
  return s;
};

/** An Ed25519 key from a 32-byte seed: `{ seed, publicKey, sign(message) }`. */
export const keyFromSeed = (seed) => {
  const pkcs8 = Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), seed]);
  const privateKey = crypto.createPrivateKey({ key: pkcs8, format: "der", type: "pkcs8" });
  const publicKey = crypto.createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32);
  return { seed, publicKey, sign: (message) => crypto.sign(null, message, privateKey) };
};

// ---- program-derived addresses ----

const P = 2n ** 255n - 19n;
const mod = (a) => ((a % P) + P) % P;
const pow = (b, e) => {
  let r = 1n;
  b = mod(b);
  while (e > 0n) { if (e & 1n) r = mod(r * b); b = mod(b * b); e >>= 1n; }
  return r;
};
const D = mod(-121665n * pow(121666n, P - 2n));

/** Whether 32 bytes are a point on the Ed25519 curve, as the runtime asks
 * of a program-derived address (which must not be one). */
export const onCurve = (bytes) => {
  const b = Buffer.from(bytes);
  b[31] &= 0x7f;
  const y = BigInt("0x" + Buffer.from(b).reverse().toString("hex"));
  if (y >= P) return false;
  const y2 = mod(y * y);
  const x2 = mod((y2 - 1n) * pow(mod(D * y2 + 1n), P - 2n));
  return x2 === 0n || pow(x2, (P - 1n) / 2n) === 1n;
};

/** `findProgramAddress`: the address and its canonical bump. */
export const findProgramAddress = (seeds, program) => {
  for (let bump = 255; bump >= 0; bump--) {
    const hash = crypto
      .createHash("sha256")
      .update(Buffer.concat([...seeds, Buffer.from([bump]), program, Buffer.from("ProgramDerivedAddress")]))
      .digest();
    if (!onCurve(hash)) return [hash, bump];
  }
  throw new Error("no bump");
};

// ---- transactions ----

/** A length as the wire writes it: 7 bits a byte, low first. */
export const shortVec = (n) => {
  const bytes = [];
  do {
    bytes.push((n & 0x7f) | (n > 0x7f ? 0x80 : 0));
    n >>= 7;
  } while (n > 0);
  return Buffer.from(bytes);
};

/** A legacy transaction with every signature slot empty. `keys` are the
 * account keys in order: `signers` of them sign, the first paying; of the
 * signers the last `readonlySigned` are read-only, and of the rest the last
 * `readonly` are. Each instruction is `[programIndex, accountIndexes, data]`. */
export const legacyTx = ({ keys, signers = 1, readonlySigned = 0, readonly = 0, blockhash, instructions }) => {
  const compiled = instructions.map(([program, indexes, data]) =>
    Buffer.concat([Buffer.from([program]), shortVec(indexes.length), Buffer.from(indexes), shortVec(data.length), data]));
  const message = Buffer.concat([
    Buffer.from([signers, readonlySigned, readonly]), shortVec(keys.length), ...keys, blockhash,
    shortVec(instructions.length), ...compiled,
  ]);
  return Buffer.concat([shortVec(signers), Buffer.alloc(64 * signers), message]);
};

/** A recent blockhash of `cluster` from its public node. Offline, a zero one. */
export const latestBlockhash = async (cluster) => {
  try {
    const res = await fetch(`https://api.${cluster}.solana.com`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getLatestBlockhash", params: [{ commitment: "finalized" }] }),
    });
    return b58decode((await res.json()).result.value.blockhash);
  } catch {
    console.log(`no ${cluster} blockhash, using a zero one`);
    return Buffer.alloc(32);
  }
};

// ---- a local node ----

/** One JSON-RPC call to `url`. Throws the node's error. */
export const rpcCall = async (url, method, params = []) => {
  const res = await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
  const body = await res.json();
  // A node says why in `data` ("Internal error", then the reason).
  if (body.error) throw new Error(`${method}: ${body.error.message}${typeof body.error.data === "string" ? ` (${body.error.data})` : ""}`);
  return body.result;
};

const UPGRADEABLE_LOADER = "BPFLoaderUpgradeab1e11111111111111111111111";

/** The programs `crates/flow-tests` runs against, which a surfpool started
 * `--offline` lacks: by address, the binary in `crates/flow-tests/programs`
 * (`node scripts/fetch-programs.mjs` downloads them, pinned by hash). */
export const PLANTED_PROGRAMS = {
  squads: [["SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf", "squads_multisig.so"]],
  programmatic: [
    ["EdSigVfK1DkeMrjFNDMjwfQaJPhPTtX7jW8uPv3oKEgN", "ed25519_signer.so"],
    ["ExecxgyHYsAXB4c5dZodV1zJZ9hqfsDCYkRDRATrpkFR", "message_executor.so"],
    ["Noncediea1fH12usShuQAz28UhgAeuE5Maf32LsMUQB", "nonce.so"],
  ],
};

/** Put `file`, a program binary of `crates/flow-tests/programs`, at
 * `address` on the surfpool node at `url`, as an upgradeable program with no
 * upgrade authority: its program account and its program data account, each
 * written with `surfnet_setAccount`. */
export const plantProgram = async (url, address, file) => {
  const path = join(root, "crates", "flow-tests", "programs", file);
  if (!existsSync(path)) throw new Error(`${path} is missing: node scripts/fetch-programs.mjs`);
  const code = readFileSync(path);
  const loader = b58decode(UPGRADEABLE_LOADER);
  const [programData] = findProgramAddress([b58decode(address)], loader);
  // UpgradeableLoaderState::ProgramData: tag 3, the slot, no authority, then
  // the code after the 45 bytes the loader keeps for that.
  const header = Buffer.alloc(45);
  header.writeUInt32LE(3, 0);
  await rpcCall(url, "surfnet_setAccount", [b58encode(programData), {
    lamports: 10_000_000_000, data: Buffer.concat([header, code]).toString("hex"), owner: UPGRADEABLE_LOADER, executable: false,
  }]);
  await rpcCall(url, "surfnet_setAccount", [address, {
    lamports: 1_141_440, data: Buffer.concat([Buffer.from([2, 0, 0, 0]), programData]).toString("hex"), owner: UPGRADEABLE_LOADER, executable: true,
  }]);
};

/** Squads v4's program config, which only its deployer can create: written
 * directly, as the flow tests do, with no creation fee and `treasury`. */
export const plantSquadsConfig = async (url, treasury) => {
  const program = b58decode("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");
  const [config] = findProgramAddress([Buffer.from("multisig"), Buffer.from("program_config")], program);
  const data = Buffer.concat([
    Buffer.from("c4d25ae790958c3f", "hex"), // Anchor's tag for ProgramConfig
    Buffer.alloc(32), // authority
    Buffer.alloc(8), // creation fee
    b58decode(treasury),
    Buffer.alloc(64),
  ]);
  await rpcCall(url, "surfnet_setAccount", [b58encode(config), {
    lamports: 10_000_000, data: data.toString("hex"), owner: b58encode(program), executable: false,
  }]);
};

/** Start `surfpool` offline on `rpcPort`, its ledger in `dir`, logging into
 * `log` (a writable stream), with each of `fund` (base58) given 100 SOL.
 * Waits until it answers and the first is funded. Returns the child and the
 * URL. */
export const startNode = async ({ dir, rpcPort, wsPort, log, fund }) => {
  mkdirSync(dir, { recursive: true });
  const node = spawn("surfpool", [
    "start", "--port", String(rpcPort), "--ws-port", String(wsPort), "--offline",
    "--no-tui", "--no-studio", "--no-deploy", "--skip-blockhash-check",
    ...fund.flatMap((address) => ["--airdrop", address]), "--airdrop-amount", String(100_000_000_000),
  ], { cwd: dir, stdio: ["ignore", "pipe", "pipe"], windowsHide: true, shell: false });
  node.stdout.pipe(log);
  node.stderr.pipe(log);
  const url = `http://127.0.0.1:${rpcPort}`;
  let up = false;
  for (let waited = 0; waited < 60_000 && !up; waited += 500) {
    try {
      await rpcCall(url, "getSlot");
      up = true;
    } catch {
      await sleep(500);
    }
  }
  if (!up) throw new Error(`surfpool on ${url} never answered`);
  for (const address of fund) {
    let lamports = 0;
    for (let waited = 0; waited < 10_000 && !lamports; waited += 500) {
      lamports = (await rpcCall(url, "getBalance", [address]).catch(() => ({ value: 0 }))).value;
      if (!lamports) await sleep(500);
    }
    // An airdrop at start may be skipped: ask for one.
    if (!lamports) {
      await rpcCall(url, "requestAirdrop", [address, 100_000_000_000]);
      await sleep(1500);
    }
  }
  return { node, url };
};

// ---- the app ----

const exe = (name) => join(root, "target", "debug", `${name}.exe`);

/** Start `target/debug/airlock.exe` with its own data folder, WebView2
 * profile and bridge pipe under `out` (emptied first), and a debugging port.
 * Returns `{ env, app, stopApp }`; pass `env` to `startHost`. */
export const startApp = (out, port) => {
  for (const name of ["airlock", "bridge-host"]) {
    if (!existsSync(exe(name))) {
      throw new Error(`${exe(name)} is missing: cargo build -p airlock -p bridge-host`);
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
  return { env, app, stopApp: () => app.kill() };
};

/** Connect to the app's page over the DevTools protocol, once it is up, and
 * put the stylesheets in, which a debug build started on its own lacks.
 * Returns `{ send, evaluate, run, shotModal, shotPage }`: `run(body)` runs
 * `body` in the page with `PAGE_HELPERS` in scope. */
export const openPage = async (port) => {
  let target;
  for (let i = 0; i < 120 && !target; i++) {
    await sleep(500);
    try {
      target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((t) => t.type === "page");
    } catch {}
  }
  if (!target) throw new Error(`no page on port ${port}`);
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  let nextId = 0;
  const pending = new Map();
  let closed = false;
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
    }
  };
  // The app went away: nothing pending will be answered.
  ws.onclose = () => {
    closed = true;
    for (const done of pending.values()) done({ error: { message: "the page closed" } });
    pending.clear();
  };
  const send = (method, params = {}) => new Promise((r) => {
    if (closed) return r({ error: { message: "the page closed" } });
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
  const css = ["tokens.css", "main.css"]
    .map((f) => readFileSync(join(root, "crates", "app-ui-kit", "assets", f), "utf8"))
    .join("\n");
  await evaluate(`(() => { const s = document.createElement('style'); s.textContent = ${JSON.stringify(css)}; document.head.appendChild(s); return true; })()`);
  const run = (body) => evaluate(`(async () => { ${PAGE_HELPERS} ${body} })()`);
  /** Screenshot the request modal, all of it. Its body scrolls inside the
   * modal, so a clip of the modal alone would cut off what is below the
   * fold: the window is made tall enough for the whole body for the shot,
   * with the body's own scrolling off, and put back after. */
  const shotModal = async (path) => {
    const need = await evaluate(`(() => {
      const m = document.querySelector('.modal.request');
      if (!m) return null;
      const body = m.querySelector('.request-body');
      const hidden = body ? body.scrollHeight - body.clientHeight : 0;
      return Math.ceil(m.getBoundingClientRect().height + Math.max(0, hidden) + 96);
    })()`);
    if (!need) return false;
    const height = Math.max(900, need);
    await send("Emulation.setDeviceMetricsOverride", { width: 1200, height, deviceScaleFactor: 1, mobile: false });
    try {
      await evaluate(`(() => {
        const m = document.querySelector('.modal.request');
        if (!m) return false;
        m.dataset.shotStyle = m.getAttribute('style') ?? '';
        m.style.maxHeight = 'none';
        const body = m.querySelector('.request-body');
        if (body) { body.dataset.shotStyle = body.getAttribute('style') ?? ''; body.style.overflow = 'visible'; body.style.flex = 'none'; }
        return true;
      })()`);
      await sleep(150);
      const r = await evaluate(`(() => { const m = document.querySelector('.modal.request'); if (!m) return null; const b = m.getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height }; })()`);
      if (!r) return false;
      const res = await send("Page.captureScreenshot", {
        format: "png",
        captureBeyondViewport: true,
        clip: { x: Math.max(0, r.x - 8), y: Math.max(0, r.y - 8), width: r.w + 16, height: r.h + 16, scale: 1 },
      });
      writeFileSync(path, Buffer.from(res.result.data, "base64"));
      return true;
    } finally {
      await evaluate(`(() => {
        for (const el of document.querySelectorAll('[data-shot-style]')) {
          if (el.dataset.shotStyle) el.setAttribute('style', el.dataset.shotStyle); else el.removeAttribute('style');
          delete el.dataset.shotStyle;
        }
        return true;
      })()`);
      await send("Emulation.setDeviceMetricsOverride", { width: 1200, height: 900, deviceScaleFactor: 1, mobile: false });
    }
  };
  /** Screenshot the window as it is. */
  const shotPage = async (path) => {
    const res = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(path, Buffer.from(res.result.data, "base64"));
  };
  return { send, evaluate, run, shotModal, shotPage, close: () => ws.close(), closed: () => closed };
};

/** Helpers in scope of every `run(body)`. */
export const PAGE_HELPERS = `
  const w = (ms) => new Promise((r) => setTimeout(r, ms));
  const visible = (el) => el && el.offsetParent !== null;
  const button = (text, root = document) => [...root.querySelectorAll('button')].find((b) => b.innerText.trim() === text && visible(b));
  const field = (text, root = document) => [...root.querySelectorAll('label')].find((l) => l.innerText.trim().startsWith(text));
  // The app's own dialog, never a site's request: one may pop up over it.
  const modal = () => [...document.querySelectorAll('.modal')].filter((m) => !m.classList.contains('request')).pop() ?? null;
  const requestModal = () => document.querySelector('.modal.request');
  const type = (el, value) => { const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value); el.dispatchEvent(new Event('input', { bubbles: true })); };
  const until = async (f, ms = 15000) => { for (let t = 0; t < ms; t += 200) { const v = f(); if (v) return v; await w(200); } return null; };
  const buttons = () => [...document.querySelectorAll('button')].filter(visible).map((b) => b.innerText.trim()).join(' | ');
  // The navigation's entry for a screen.
  const nav = (text) => [...document.querySelectorAll('button, a')].find((b) => b.innerText.trim() === text && visible(b) && !b.closest('.content'));
  const modalButtons = () => [...document.querySelectorAll('.modal.request .btn-row button')].filter(visible)
    .map((b) => b.innerText.trim() + (b.className ? ' [' + b.className + ']' : '') + (b.disabled ? ' (disabled)' : ''));
  // The card on Wallets of the wallet labelled so.
  // Its name is the first text of the card's heading, before its chips.
  const walletCard = (label) => [...document.querySelectorAll('li.wallet-row')].find((li) => li.querySelector('h3')?.firstChild?.textContent?.trim() === label);
  // A wallet's Details or Settings: its icon button, or on a phone the item
  // of the sheet its card's More actions opens. Null when it has none.
  const walletDialogButton = async (label, which) => {
    const find = () => document.querySelector('button[aria-label=' + JSON.stringify(which + ' of ' + label) + ']');
    if (!await until(() => find() || walletCard(label))) return null;
    if (find()) return find();
    const more = walletCard(label)?.querySelector('button.wallet-more');
    if (!more) return null;
    more.click();
    return await until(find, 5000);
  };
`;

/** Create a vault and add a wallet from `key`'s seed, labelled `label`.
 * Answers "ok" or what went wrong. */
export const addWallet = (page, key, label = "Main") => page.run(`
  for (const p of document.querySelectorAll('input[type=password]')) type(p, 'correct horse battery');
  await w(200);
  button('Create vault')?.click();
  // A later wallet: from wherever the window is, go to Wallets.
  if (!await until(() => button('Add wallet') || (!button('Create vault') && nav('Wallets')), 15000)) return 'no Add wallet: ' + buttons();
  if (!button('Add wallet')) nav('Wallets').click();
  if (!await until(() => button('Add wallet'))) return 'no Add wallet: ' + buttons();
  button('Add wallet').click();
  await w(500);
  button('Private key', modal())?.click();
  await w(300);
  type(field('Label', modal()).querySelector('input'), ${JSON.stringify(label)});
  type(field('Private key', modal()).querySelector('textarea'), '${key.seed.toString("hex")}');
  await w(300);
  button('Save wallet', modal()).click();
  await w(1500);
  return document.body.innerText.includes('${b58encode(key.publicKey).slice(0, 4)}') ? 'ok' : 'not seen: ' + buttons();
`);

// ---- bridge-host ----

/** Start `bridge-host` with the app's `env` and speak native messaging to
 * it: frames of a 32-bit little-endian length, then the JSON. Returns
 * `{ frame(obj), replyTo(id, ms), frames, since(n), stopHost }`: `frames`
 * holds every frame that came back, replies and pushes, in order. */
export const startHost = (env) => {
  const host = spawn(exe("bridge-host"), [], { env, stdio: ["pipe", "pipe", "ignore"] });
  let buffer = Buffer.alloc(0);
  const frames = [];
  host.stdout.on("data", (d) => {
    buffer = Buffer.concat([buffer, d]);
    while (buffer.length >= 4) {
      const len = buffer.readUInt32LE(0);
      if (buffer.length < 4 + len) break;
      frames.push(JSON.parse(buffer.subarray(4, 4 + len).toString("utf8")));
      buffer = buffer.subarray(4 + len);
    }
  });
  const frame = (obj) => {
    const body = Buffer.from(JSON.stringify(obj));
    const head = Buffer.alloc(4);
    head.writeUInt32LE(body.length);
    host.stdin.write(Buffer.concat([head, body]));
  };
  const replyTo = async (id, ms = 20000) => {
    for (let t = 0; t < ms; t += 100) {
      const reply = frames.find((r) => r.id === id);
      if (reply) return reply;
      await sleep(100);
    }
    return null;
  };
  const since = (n) => frames.slice(n);
  const stopHost = () => {
    host.stdin.end();
    host.kill();
  };
  return { frame, replyTo, frames, since, stopHost };
};

/** Connect `origin` with the current wallet, approving the modal. Answers
 * the accounts the site got. */
export const connectSite = async (page, host, origin) => {
  host.frame({
    v: 1, id: "connect-1", origin, method: "connect",
    expiresAt: Math.floor(Date.now() / 1000) + 600, params: {},
  });
  await page.run(`await until(() => requestModal()); await w(1000); button('Approve')?.click(); await w(800); return 1;`);
  return (await host.replyTo("connect-1", 5000))?.result?.accounts ?? [];
};

/** Point the app at a custom RPC in Settings, named `name`. Answers "ok" or
 * what went wrong. Settings is left open. */
export const setCustomRpc = (page, url, name = "local") => page.run(`
  const settings = nav('Settings');
  if (!settings) return 'no Settings in the navigation: ' + buttons();
  settings.click();
  const select = await until(() => [...document.querySelectorAll('select')].find((s) => [...s.options].some((o) => o.value === 'custom')));
  if (!select) return 'no cluster list';
  select.value = 'custom';
  select.dispatchEvent(new Event('change', { bubbles: true }));
  const urlInput = await until(() => field('RPC URL')?.querySelector('input'));
  if (!urlInput) return 'no RPC URL field';
  type(urlInput, ${JSON.stringify(url)});
  type(field('Name').querySelector('input'), ${JSON.stringify(name)});
  await w(300);
  const use = await until(() => button('Use this RPC'));
  if (!use) return 'no Use this RPC';
  use.click();
  const done = await until(() => document.querySelector('.rpc-in-use')?.innerText.includes(${JSON.stringify(url)}) && !button('Use this RPC'), 15000);
  return done ? 'ok' : 'not in use: ' + (document.querySelector('.rpc-in-use')?.innerText ?? '');
`);

/** Go to Wallets. Answers "ok" or what went wrong. */
const TO_WALLETS = `
  if (modal()) button('Cancel', modal())?.click() ?? button('Close', modal())?.click();
  if (!button('Add wallet')) nav('Wallets')?.click();
  if (!await until(() => button('Add wallet'))) return 'no Wallets: ' + buttons();
`;

/** Make the wallet labelled `label` the current one, in the top bar's
 * switcher. Answers "ok" or what went wrong. */
export const selectWallet = (page, label) => page.run(`
  const select = [...document.querySelectorAll('select')].find((s) => visible(s) && [...s.options].some((o) => o.text.trim() === ${JSON.stringify(label)}) && s.getBoundingClientRect().top < 60);
  if (!select) return 'no wallet switcher with ' + ${JSON.stringify(label)};
  const option = [...select.options].find((o) => o.text.trim() === ${JSON.stringify(label)});
  select.value = option.value;
  select.dispatchEvent(new Event('change', { bubbles: true }));
  await w(500);
  return select.options[select.selectedIndex]?.text.trim() === ${JSON.stringify(label)} ? 'ok' : 'not selected';
`);

/** On Review and Sign, press the button that signs and submits, and wait
 * until the transaction landed. Answers "ok" or what went wrong. */
export const signAndSubmit = (page) => page.run(`
  if (!document.querySelector('.content')?.innerText.startsWith('Review and Sign')) nav('Review and Sign')?.click();
  const words = (b) => b.innerText.replace(/\\s+/g, ' ').trim();
  const isSubmit = (b) => /^Sign transaction( \\(\\d+\\))? and submit$|^Submit$/.test(words(b));
  const press = await until(() => [...document.querySelectorAll('button')].find((b) => visible(b) && !b.disabled && isSubmit(b)), 60000);
  if (!press) return 'no Sign transaction and submit: ' + [...document.querySelectorAll('button')].filter((b) => visible(b) && isSubmit(b)).map((b) => JSON.stringify(words(b)) + (b.disabled ? ' (disabled)' : '')).join(', ') + ' | ' + buttons();
  // Wait out the simulation, which Sign waits on.
  await until(() => !document.body.innerText.includes('Simulating'), 30000);
  await w(1000);
  press.click();
  const landed = await until(() => /It landed|It failed|could not|refused/i.test((document.querySelector('.content')?.innerText ?? '').split('Edit on Build').pop()), 60000);
  const after = (document.querySelector('.content')?.innerText ?? '').split('Edit on Build').pop().trim();
  return landed && after.includes('It landed') ? 'ok' : 'not landed: ' + after.slice(0, 400);
`);

/** Add an offline wallet: `address`, a key this vault does not hold.
 * Answers "ok" or what went wrong. */
export const addOfflineWallet = (page, label, address) => page.run(`
  ${TO_WALLETS}
  button('Add wallet').click();
  await w(500);
  button('Existing wallet', modal())?.click();
  await w(200);
  button('Offline', modal()).click();
  await w(400);
  const m = modal();
  type(field('Label', m).querySelector('input'), ${JSON.stringify(label)});
  const input = [...m.querySelectorAll('input, textarea')].find((el) => el !== field('Label', m).querySelector('input') && visible(el) && el.type !== 'checkbox');
  if (!input) return 'no address field: ' + m.innerText;
  type(input, ${JSON.stringify(address)});
  await w(300);
  const save = [...m.querySelectorAll('.btn-row button.primary')].find((b) => visible(b) && !b.disabled);
  if (!save) return 'no save: ' + m.innerText;
  save.click();
  await until(() => !modal(), 10000);
  await w(500);
  // A phone shows the address shortened, in its card's title.
  const card = walletCard(${JSON.stringify(label)});
  const shown = document.body.innerText.includes(${JSON.stringify(address)}) || card?.querySelector('code')?.title === ${JSON.stringify(address)};
  return shown ? 'ok' : 'not seen: ' + (modal()?.innerText ?? buttons());
`);

/** Create a Squads v4 multisig through the app, as a person does: Add
 * wallet, Create new, Squads, the members by label, the threshold and the
 * payer; then sign and submit it on Review and Sign. Answers
 * `{ vault, multisig }` or `{ error }`. */
export const createMultisig = async (page, { label, members, threshold, payer }) => {
  const opened = await page.run(`
    ${TO_WALLETS}
    button('Add wallet').click();
    await w(500);
    button('Create new', modal()).click();
    await w(300);
    button('Squads', modal()).click();
    await w(500);
    const m = modal();
    type(field('Label', m).querySelector('input'), ${JSON.stringify(label)});
    for (const li of m.querySelectorAll('.derive-row')) {
      const name = li.querySelector('.addr-label')?.innerText.trim();
      const box = li.querySelector('input[type=checkbox]');
      if (${JSON.stringify(members)}.includes(name) !== box.checked) box.click();
    }
    type(field('Approvals needed', m).querySelector('input'), ${JSON.stringify(String(threshold))});
    m.querySelector('.picker-button').click();
    await w(300);
    const item = [...document.querySelectorAll('.picker-menu .picker-item')].find((b) => b.querySelector('.addr-label')?.innerText.trim() === ${JSON.stringify(payer)});
    if (!item) return 'no payer ' + ${JSON.stringify(payer)};
    item.click();
    await w(300);
    button('Create multisig', m).click();
    if (!await until(() => !modal(), 20000)) return 'the form stayed: ' + (modal()?.querySelector('.error')?.innerText ?? '');
    return 'ok';
  `);
  if (opened !== "ok") return { error: opened };
  const submitted = await signAndSubmit(page);
  if (submitted !== "ok") return { error: submitted };
  return readMultisig(page, label);
};

/** The vault and multisig addresses of the multisig labelled `label`, from
 * its Details on Wallets. */
export const readMultisig = async (page, label) => {
  const read = await page.run(`
    ${TO_WALLETS}
    const details = await walletDialogButton(${JSON.stringify(label)}, 'Details');
    if (!details) return 'no Details of ' + ${JSON.stringify(label)};
    details.click();
    await w(600);
    const m = modal();
    const codes = [...m.querySelectorAll('code')].map((c) => c.innerText.trim()).filter((t) => t.length >= 32);
    button('Close', m)?.click();
    await w(300);
    return { vault: codes[0], multisig: codes[1] };
  `);
  return typeof read === "string" ? { error: read } : read;
};

/** Change the Settings of the multisig labelled `label`: the member it
 * proposes and votes as (`actingAs`, a label), and the wallets ticked under
 * "Approves together with" (`together`, labels). What is left undefined
 * stays. Answers "ok" or what went wrong. */
export const multisigSettings = (page, label, { actingAs, together }) => page.run(`
  ${TO_WALLETS}
  const gear = await walletDialogButton(${JSON.stringify(label)}, 'Settings');
  if (!gear) return 'no Settings of ' + ${JSON.stringify(label)};
  gear.click();
  await w(600);
  const m = modal();
  const acting = ${JSON.stringify(actingAs ?? null)};
  if (acting) {
    const pickerLabel = field('Proposes and votes as', m);
    if (!pickerLabel) return 'no acting member picker: ' + m.innerText;
    pickerLabel.querySelector('.picker-button').click();
    await w(300);
    const item = [...document.querySelectorAll('.picker-menu .picker-item')].find((b) => b.querySelector('.addr-label')?.innerText.trim() === acting);
    if (!item) return 'no member ' + acting;
    item.click();
    await w(300);
  }
  const together = ${JSON.stringify(together ?? null)};
  if (together) {
    for (const box of m.querySelectorAll('input[type=checkbox]')) {
      const row = box.closest('li, label, .derive-row, div');
      const name = row?.querySelector('.addr-label')?.innerText.trim();
      if (name && together.includes(name) !== box.checked) box.click();
    }
    await w(300);
  }
  const chosen = acting && field('Proposes and votes as', m)?.querySelector('.picker-button')?.innerText.replace(/\\s+/g, ' ').trim();
  if (acting && !chosen?.startsWith(acting)) return 'the acting member reads ' + chosen + ', not ' + acting;
  button('Save', m).click();
  if (!await until(() => !modal(), 10000)) return 'Settings stayed open: ' + (modal()?.querySelector('.error')?.innerText ?? modal()?.innerText.slice(0, 300));
  await w(400);
  // Read back from Details what was saved.
  const details = await walletDialogButton(${JSON.stringify(label)}, 'Details');
  details.click();
  await w(500);
  const text = modal()?.innerText ?? '';
  button('Close', modal())?.click();
  await w(300);
  if (acting && !new RegExp('Proposes and votes as\\\\s+' + acting).test(text)) return 'Details say: ' + text.replace(/\\s+/g, ' ').slice(0, 400);
  return 'ok';
`);

/** On the Proposals of the multisig labelled `label`, press `action`
 * ("Approve and execute", "Approve", "Execute") on the newest proposal that
 * offers it, then sign and submit on Review and Sign. Answers "ok" or what
 * went wrong. */
export const actOnProposal = async (page, label, action, shoot = async () => {}, beforeSubmit = async () => "ok") => {
  const opened = await page.run(`
    ${TO_WALLETS}
    // Proposals is beside the card's buttons on a desktop, in its foot on a phone.
    const card = await until(() => walletCard(${JSON.stringify(label)}));
    const open = card && [...card.querySelectorAll('button')].find((b) => b.innerText.trim() === 'Proposals');
    if (!open) return 'no Proposals for ' + ${JSON.stringify(label)};
    open.click();
    const list = await until(() => document.querySelector('.modal.proposals .book-list'), 30000);
    if (!list) return 'no proposals listed: ' + (modal()?.innerText ?? '');
    // The oldest proposal that offers it: a multisig's proposals run in the
    // order they were made. A row opens when its head is clicked.
    const offers = (row) => [...row.querySelectorAll('button')].find((b) => visible(b) && !b.disabled && b.innerText.trim() === ${JSON.stringify(action)});
    const number = (row) => Number(row.querySelector('.proposal-number')?.innerText.replace('#', '') ?? 0);
    const rows = [...list.querySelectorAll('.proposal-row')].sort((a, b) => number(a) - number(b));
    let picked = null;
    for (const row of rows) {
      if (!offers(row)) {
        const head = row.querySelector('.proposal-head');
        if (head?.getAttribute('aria-expanded') !== 'true') head?.click();
        await until(() => offers(row), 1500);
      }
      if (offers(row)) {
        picked = offers(row);
        break;
      }
    }
    if (!picked) return 'no ' + ${JSON.stringify(action)} + ': ' + document.querySelector('.modal.proposals').innerText.slice(0, 800);
    document.querySelectorAll('[data-drive-pick]').forEach((b) => b.removeAttribute('data-drive-pick'));
    picked.setAttribute('data-drive-pick', '1');
    picked.scrollIntoView({ block: 'center' });
    return 'ok';
  `);
  if (opened !== "ok") return opened;
  await shoot("proposals");
  const pressed = await page.run(`
    const press = document.querySelector('.modal.proposals button[data-drive-pick]');
    if (!press) return 'no ' + ${JSON.stringify(action)} + ' any more';
    press.click();
    if (!await until(() => !document.querySelector('.modal.proposals'), 30000)) return 'the proposals stayed: ' + document.querySelector('.modal.proposals').innerText.slice(-400);
    return 'ok';
  `);
  if (pressed !== "ok") return pressed;
  await sleep(1500);
  await shoot("review-and-sign");
  // The transaction is compiled: what changes now does not change it.
  const before = await beforeSubmit();
  if (before !== "ok") return before;
  return signAndSubmit(page);
};

/** Stop every process whose command line holds `folder`, but this one: an
 * app's WebView2 and a headless browser outlive a kill of the process Node
 * started, and hold their profile folder. */
export const stopByFolder = (folder) => {
  if (process.platform !== "win32") return;
  const quoted = folder.replace(/'/g, "''");
  const script = `Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne ${process.pid} -and $_.CommandLine -and $_.CommandLine.Contains('${quoted}') } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`;
  try {
    execFileSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], { stdio: "ignore", timeout: 60_000 });
  } catch {
    // Nothing left, or PowerShell failed: removing the folder says which.
  }
};

/** Stop `child` and everything it started. */
export const stopTree = (child) => {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  if (process.platform === "win32") {
    try {
      execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore", timeout: 30_000 });
      return;
    } catch {
      // Gone already, or taskkill failed: kill what Node holds.
    }
  }
  child.kill();
};

// ---- the approver: the person in the app's window ----

/** What the approver does by default, for each kind of request: approve a
 * connect, sign a transaction (Review, then sign on Review and Sign, when
 * the modal offers no Sign) and sign a message. A scenario may say
 * "reject" for any of them. */
export const APPROVE_ALL = { connect: "approve", transaction: "sign", message: "sign" };

/** Where the app's window stands, read in one go. */
const WINDOW_STATE = `(() => {
  const visible = (el) => el && el.offsetParent !== null;
  const words = (el) => (el?.innerText ?? '').replace(/\\s+/g, ' ').trim();
  const list = (root) => [...root.querySelectorAll('button')].filter(visible)
    .map((b) => ({ text: b.innerText.trim(), disabled: b.disabled, cls: b.className }));
  // A modal with a password field: the vault asks to be unlocked.
  const unlock = [...document.querySelectorAll('.modal')].find((m) => !m.classList.contains('request') && visible(m.querySelector('input[type=password]')));
  if (unlock) return { where: 'password', text: words(unlock), buttons: list(unlock) };
  const m = document.querySelector('.modal.request');
  if (m) {
    const text = m.innerText;
    return {
      where: 'modal',
      code: words(m.querySelector('.chip.kind code')),
      title: words(m.querySelector('h3')),
      text: words(m),
      busy: /Looking for the IDLs|Simulating…|Preparing/.test(text),
      error: words(m.querySelector('.request-body .error, .error[role=alert]')),
      buttons: list(m.querySelector('.btn-row:last-of-type') ?? m),
    };
  }
  const toast = document.querySelector('.request-toast');
  if (visible(toast)) return { where: 'toast', code: words(toast.querySelector('.chip.kind code')), text: words(toast) };
  return { where: 'none' };
})()`;

/** Click the visible button labelled `text` inside `scope` (a selector). */
const clickButton = (page, scope, text) => page.evaluate(`(() => {
  const root = document.querySelector(${JSON.stringify(scope)});
  const b = root && [...root.querySelectorAll('button')].find((b) => b.innerText.trim() === ${JSON.stringify(text)} && b.offsetParent !== null && !b.disabled);
  if (!b) return false;
  b.click();
  return true;
})()`);

const slug = (text) => text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 48) || "request";

/**
 * Play the person in the app's window: watch it over the DevTools protocol
 * and, whenever a request modal is up, do what `scenarios` says for it. A
 * scenario is picked by `current()`, the name of what is being run now (a
 * page run's id), and maps a kind of request (`connect`, `transaction`,
 * `message`) to `approve`, `sign`, `review` (a transaction: Review, then
 * sign on Review and Sign, even when the modal offers Sign) or `reject`;
 * what it leaves out is `APPROVE_ALL`'s. The vault's password is typed when
 * it is asked for.
 *
 * Each modal it acts on is screenshot into `dir` as
 * `<current>-<n>-<kind>.png`, and each action logged through `log`.
 * Returns `{ stop(), actions, problems }`: `stop()` waits for the loop to
 * end; `problems` lists what it could not get past.
 */
export const startApprover = ({ page, dir, scenarios = {}, current = () => "request", password = "correct horse battery", log = console.log }) => {
  mkdirSync(dir, { recursive: true });
  const actions = [];
  const problems = [];
  // By request code: when it was first seen, and what was done.
  const seen = new Map();
  let stopped = false;
  let paused = false;
  let shots = 0;

  const kindOf = (state) => {
    if (state.buttons.some((b) => b.text === "Approve")) return "connect";
    if (/^Sign (message|in)$/.test(state.title)) return "message";
    return "transaction";
  };
  const button = (state, ...texts) => state.buttons.find((b) => texts.includes(b.text) || texts.some((t) => t instanceof RegExp && t.test(b.text)));
  const shoot = async (name) => {
    const file = join(dir, `${String(++shots).padStart(2, "0")}-${name}.png`);
    if (await page.shotModal(file)) return file;
    await page.shotPage(file);
    return file;
  };
  const record = (entry) => {
    actions.push({ at: new Date().toISOString(), ...entry });
    log(`approver: ${entry.run} ${entry.kind} ${entry.code ?? ""} "${entry.title ?? ""}" -> ${entry.pressed}${entry.note ? ` (${entry.note})` : ""}`);
  };

  const step = async () => {
    const state = await page.evaluate(WINDOW_STATE);
    if (!state) return;
    if (state.where === "password") {
      const run = current();
      const filled = await page.evaluate(`(() => {
        const m = [...document.querySelectorAll('.modal')].find((m) => !m.classList.contains('request') && m.querySelector('input[type=password]'));
        const input = m.querySelector('input[type=password]');
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(password)});
        input.dispatchEvent(new Event('input', { bubbles: true }));
        return true;
      })()`);
      await sleep(300);
      const pressed = await page.evaluate(`(() => {
        const m = [...document.querySelectorAll('.modal')].find((m) => !m.classList.contains('request') && m.querySelector('input[type=password]'));
        const b = [...m.querySelectorAll('button')].find((b) => b.offsetParent !== null && !b.disabled && /unlock|confirm|continue|ok/i.test(b.innerText));
        if (!b) return null;
        b.click();
        return b.innerText.trim();
      })()`);
      record({ run, kind: "password", pressed: pressed ?? "nothing", note: filled ? "typed the vault's password" : "no field" });
      await sleep(1000);
      return;
    }
    // A request of an app on a phone shows no code: one is told from the
    // next by its title, and by the modal going away between them.
    if (state.where !== "modal") {
      for (const key of [...seen.keys()]) if (key.startsWith("no code:")) seen.delete(key);
    }
    if (state.where === "toast") {
      // A request set aside, the modal closed: open it again.
      if (await clickButton(page, ".request-toast", "Open")) await sleep(500);
      return;
    }
    if (state.where !== "modal") return;

    const now = Date.now();
    const key = state.code || `no code: ${state.title}`;
    const entry = seen.get(key) ?? { first: now, acted: 0, reviewed: false, stuckShot: false };
    seen.set(key, entry);
    const run = current();
    const kind = kindOf(state);
    const wanted = { ...APPROVE_ALL, ...(scenarios[run] ?? {}) }[kind];
    // Acted on and still up: wait for it to go, and say so once if it stays.
    if (entry.acted && now - entry.actedAt < 20_000) return;
    if (entry.acted && !entry.stuckShot) {
      entry.stuckShot = true;
      const file = await shoot(`${slug(run)}-stuck`);
      problems.push(`${run}: the ${kind} request ${state.code} stayed up 20 s after ${entry.pressed}: ${state.error || state.text.slice(-300)} (${file})`);
      log(`approver: ${problems.at(-1)}`);
      return;
    }
    if (entry.acted) return;

    if (wanted === "reject") {
      const b = button(state, "Reject");
      if (!b) return;
      const file = await shoot(`${slug(run)}-${kind}`);
      await clickButton(page, ".modal.request", "Reject");
      Object.assign(entry, { acted: 1, actedAt: Date.now(), pressed: "Reject" });
      record({ run, kind, code: state.code, title: state.title, pressed: "Reject", screenshot: file });
      return;
    }
    if (kind === "connect" || kind === "message") {
      const text = kind === "connect" ? "Approve" : "Sign";
      const b = button(state, text);
      if (!b || b.disabled || state.busy) return; // not armed yet
      const file = await shoot(`${slug(run)}-${kind}`);
      await clickButton(page, ".modal.request", text);
      Object.assign(entry, { acted: 1, actedAt: Date.now(), pressed: text });
      record({ run, kind, code: state.code, title: state.title, pressed: text, screenshot: file });
      return;
    }
    // A transaction: Unlock when the vault is locked, Sign once it is armed
    // and nothing is still being looked up, else Review.
    if (button(state, "Unlock")) {
      await clickButton(page, ".modal.request", "Unlock");
      record({ run, kind, code: state.code, title: state.title, pressed: "Unlock" });
      await sleep(800);
      return;
    }
    if (state.busy) return;
    const sign = button(state, /^Sign( and send)?( all \d+)?$/);
    if (sign && !sign.disabled && wanted !== "review" && wanted !== "review-only") {
      const file = await shoot(`${slug(run)}-${kind}`);
      await clickButton(page, ".modal.request", sign.text);
      Object.assign(entry, { acted: 1, actedAt: Date.now(), pressed: sign.text });
      record({ run, kind, code: state.code, title: state.title, pressed: sign.text, screenshot: file });
      return;
    }
    // Sign stays off for a moment after the modal is ready (800 ms), and a
    // little longer while preparing settles. Past that, take Review.
    if (!entry.readyAt) entry.readyAt = now;
    const review = button(state, "Review", "Review anyway");
    // Asked for Review: once Sign is armed too, so the modal is shot ready.
    const waitFor = wanted === "review" ? (sign && !sign.disabled ? 0 : 4000) : wanted === "review-only" ? 1500 : 4000;
    if (now - entry.readyAt < waitFor || !review || review.disabled) {
      if (now - entry.first > 120_000 && !entry.stuckShot) {
        entry.stuckShot = true;
        const file = await shoot(`${slug(run)}-no-way-forward`);
        problems.push(`${run}: the ${kind} request ${state.code} offers no way forward after 2 minutes: ${state.buttons.map((b) => b.text + (b.disabled ? " (off)" : "")).join(", ")} (${file})`);
        log(`approver: ${problems.at(-1)}`);
      }
      return;
    }
    const file = await shoot(`${slug(run)}-${kind}-review`);
    await clickButton(page, ".modal.request", review.text);
    Object.assign(entry, { acted: 1, actedAt: Date.now(), pressed: review.text });
    record({ run, kind, code: state.code, title: state.title, pressed: review.text, screenshot: file, note: wanted === "review" ? "the scenario asks for Review" : wanted === "review-only" ? "the scenario signs elsewhere" : "the modal offered no Sign" });
    // Another device signs: Review and Sign is left to whoever plays it.
    if (wanted === "review-only") {
      await sleep(2000);
      await page.shotPage(join(dir, `${String(++shots).padStart(2, "0")}-${slug(run)}-review-and-sign.png`));
      return;
    }
    await signOnReview(run, state.code);
    // The next request may be up already, so the modal was never seen gone:
    // a request with no code is done with here.
    if (!state.code) seen.delete(key);
  };

  /** After Review: sign on Review and Sign, which the request opened. Shot
   * before signing, and once more after, with the steps where they ended. */
  const signOnReview = async (run, code) => {
    let presses = 0;
    // Not a button of a request modal: on a phone the next request's modal
    // sits inside the page, and its Sign is that request's, not this one's.
    const signButton = `[...(document.querySelector('.content') ?? document).querySelectorAll('button')].find((b) => b.offsetParent !== null && !b.disabled && !b.closest('nav') && !b.closest('.modal') && /^Sign( transaction)?( and (send|submit))?/.test(b.innerText.trim()))`;
    const modalUp = `[...document.querySelectorAll('.modal.request')].some((m) => m.offsetParent !== null)`;
    for (let waited = 0; waited < 60_000; waited += 500) {
      await sleep(500);
      // The next request came: it is the main loop's, by its own name.
      if (presses > 0 && (await page.evaluate(modalUp))) return;
      const ready = await page.evaluate(`(() => { const b = ${signButton}; return b ? b.innerText.trim() : null; })()`);
      // Wait out the simulation and the button's arming before the shot.
      if (!ready || (await page.evaluate(`document.body.innerText.includes('Simulating')`))) continue;
      await sleep(1000);
      await page.shotPage(join(dir, `${String(++shots).padStart(2, "0")}-${slug(run)}-review-and-sign.png`));
      const pressed = await page.evaluate(`(() => { const b = ${signButton}; if (!b) return null; b.click(); return b.innerText.trim(); })()`);
      if (!pressed) continue;
      record({ run, kind: "transaction", code, pressed: `${pressed} (Review and Sign)` });
      await sleep(2500);
      await page.shotPage(join(dir, `${String(++shots).padStart(2, "0")}-${slug(run)}-review-and-sign-after.png`));
      // An authorization is signed on one press and the transaction on the
      // next, and a batch moves on to its next transaction: go on while the
      // screen offers another press.
      presses += 1;
      const more = presses < 32 && (await page.run(`return !!(await until(() => ${signButton} || ${modalUp}, 5000)) && !(${modalUp});`));
      if (more) continue;
      return;
    }
    const file = await shoot(`${slug(run)}-review-no-sign`);
    problems.push(`${run}: Review and Sign offered no Sign for ${code} (${file})`);
  };

  const loop = (async () => {
    while (!stopped && !page.closed?.()) {
      try {
        if (!paused) await step();
      } catch (err) {
        problems.push(`approver: ${err.message}`);
        log(`approver error: ${err.message}`);
      }
      await sleep(250);
    }
  })();
  return {
    actions,
    problems,
    stop: async () => {
      stopped = true;
      await loop;
    },
    // While someone else uses the window (another member, another device),
    // the person stands back.
    pause: () => {
      paused = true;
    },
    resume: () => {
      paused = false;
    },
  };
};
