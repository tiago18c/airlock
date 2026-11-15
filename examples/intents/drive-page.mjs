// Drives the test page in headless Edge or Chrome over the DevTools protocol:
// it serves dist/, opens the page with its dev transport pointed at a relay,
// connects, picks an account, sets the RPC and chain, then presses each row's
// button in turn and waits for the row to settle. It writes a screenshot of the
// whole page, light and dark, and outcome.json with each row's outcome and the
// event log.
//
//   node drive-page.mjs <outdir> (--relay <port> | --fake) [options]
//
//   --relay <port>       a relay already running (relay.mjs) on 127.0.0.1
//   --fake               start a relay of its own, with test/fake-app.mjs as the
//                        app and its node, so nothing else need run
//   --only <ids>         runs to make, by id or row key, comma separated (all by default)
//   --rpc <url>          the node the page reads and sends through (default
//                        http://127.0.0.1:8899, or the fake's)
//   --chain <id>         solana:devnet (default), solana:testnet or solana:mainnet
//   --account <i|label>  the account to act as, by its index or its label (default 0)
//   --timeout <seconds>  how long one row, or the connect, may wait (default 300)
//   --browser <path>     Edge or Chrome, when not where it usually is (or BROWSER)
//   --airdrop            ask the node for 1 SOL for the app key before the rows
//                        (a local validator; a public faucet often refuses)
//
// The page only waits: for the app to answer and for the person to decide.
// It never assumes how long either takes. Everything stays on 127.0.0.1.
//
// Exit code: 0 when every run passed, 1 when one did not, 2 when the run
// could not be made.

import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { logLine, startRelay } from "./relay.mjs";
import { DIST, serve } from "./serve.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const CHAINS = ["solana:devnet", "solana:testnet", "solana:mainnet"];

/** Every run, in the page's order. `set` puts a row's options before it runs. */
export const RUNS = [
  { id: "profile", row: "profile" },
  { id: "memo", row: "memo" },
  { id: "transfer", row: "transfer" },
  { id: "mint:classic", row: "mint", set: { token: "classic" } },
  { id: "mint:token-2022", row: "mint", set: { token: "token-2022" } },
  { id: "cosign:kept", row: "cosign", set: { replaceable: false } },
  { id: "cosign:replaceable", row: "cosign", set: { replaceable: true } },
  { id: "return", row: "return" },
  { id: "order:serial", row: "order", set: { order: "serial" } },
  { id: "order:any", row: "order", set: { order: "any" } },
  { id: "order:atomic", row: "order", set: { order: "atomic" } },
  { id: "lying", row: "lying" },
  { id: "account-note", row: "account-note" },
  { id: "message:auto", row: "message", set: { mode: "" } },
  { id: "message:signature", row: "message", set: { mode: "signature" } },
  { id: "message:memo", row: "message", set: { mode: "memo" } },
  { id: "fallback", row: "fallback" },
];

/** The runs `--only` names: a run by its id, or every run of a row by the row's key. */
export function selectRuns(only) {
  if (!only) return RUNS;
  const wanted = only.split(/[,\s]+/).filter(Boolean);
  const unknown = wanted.filter((name) => !RUNS.some((run) => run.id === name || run.row === name));
  if (unknown.length) throw new Error(`No run is called ${unknown.join(", ")}. The runs: ${RUNS.map((run) => run.id).join(", ")}.`);
  return RUNS.filter((run) => wanted.includes(run.id) || wanted.includes(run.row));
}

export function parseArgs(argv) {
  const options = { outdir: null, relay: null, fake: false, airdrop: false, only: null, rpc: null, chain: "solana:devnet", account: "0", timeout: 300, browser: process.env.BROWSER ?? null };
  const takes = new Set(["--relay", "--only", "--rpc", "--chain", "--account", "--timeout", "--browser"]);
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--fake") options.fake = true;
    else if (arg === "--airdrop") options.airdrop = true;
    else if (takes.has(arg)) {
      const value = argv[++i];
      if (value === undefined) throw new Error(`${arg} needs a value.`);
      options[arg.slice(2)] = value;
    } else if (arg.startsWith("--")) throw new Error(`Unknown option ${arg}.`);
    else if (options.outdir === null) options.outdir = arg;
    else throw new Error(`One output folder only, not also ${arg}.`);
  }
  if (!options.outdir) throw new Error("Give an output folder.");
  if (options.fake === (options.relay !== null)) throw new Error("Give either --relay <port> or --fake.");
  if (options.relay !== null) {
    options.relay = Number(options.relay);
    if (!Number.isInteger(options.relay) || options.relay <= 0 || options.relay > 65535) throw new Error("--relay takes a port.");
  }
  if (!CHAINS.includes(options.chain)) throw new Error(`--chain is one of ${CHAINS.join(", ")}.`);
  options.timeout = Number(options.timeout);
  if (!(options.timeout > 0)) throw new Error("--timeout takes a number of seconds.");
  if (options.rpc !== null) {
    const url = new URL(options.rpc);
    if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error("--rpc takes an http:// or https:// URL.");
  }
  selectRuns(options.only);
  return options;
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

function freePort() {
  return new Promise((done, fail) => {
    const server = createServer();
    server.once("error", fail);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => done(port));
    });
  });
}

/* The browser */

function findBrowser(given) {
  if (given) {
    if (!existsSync(given)) throw new Error(`No browser at ${given}.`);
    return given;
  }
  const local = process.env.LOCALAPPDATA ?? "";
  const candidates =
    process.platform === "win32"
      ? [
          "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
          "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
          join(local, "Google\\Chrome\\Application\\chrome.exe"),
          "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        ]
      : process.platform === "darwin"
        ? ["/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
        : ["/usr/bin/microsoft-edge", "/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
  const found = candidates.find((path) => existsSync(path));
  if (!found) throw new Error("Found neither Edge nor Chrome. Give one with --browser <path>.");
  return found;
}

function browserArgs(profile) {
  return [
    "--headless=new",
    // Port 0: the browser takes a free one and writes it to DevToolsActivePort.
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-extensions",
    // Nothing of the browser's own off the machine while the page runs.
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-sync",
    "--no-pings",
    "about:blank",
  ];
}

/** Stops every process started with this profile folder. A headless Edge
 * started from Node can outlive a kill of the process Node holds. */
function killProfile(profile) {
  if (process.platform === "win32") {
    const quoted = profile.replace(/'/g, "''");
    const script = `Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -and $_.CommandLine.Contains('${quoted}') } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`;
    try {
      execFileSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], { stdio: "ignore", timeout: 30_000 });
    } catch {
      // Nothing left to stop, or PowerShell itself failed; the folder removal below says which.
    }
  } else {
    try {
      execFileSync("pkill", ["-f", profile], { stdio: "ignore" });
    } catch {
      // Nothing matched.
    }
  }
}

async function devToolsPort(profile, ms) {
  const file = join(profile, "DevToolsActivePort");
  for (let waited = 0; waited < ms; waited += 250) {
    if (existsSync(file)) {
      const port = Number(readFileSync(file, "utf8").split("\n")[0]);
      if (port > 0) return port;
    }
    await sleep(250);
  }
  return null;
}

/** Starts the browser and returns its debugging port. Tries a second way on
 * Windows, where an Edge spawned from Node has once never opened the port. */
async function launchBrowser(exe, profile) {
  const child = spawn(exe, browserArgs(profile), { stdio: "ignore", detached: false, windowsHide: true });
  child.on("error", () => {});
  let port = await devToolsPort(profile, 20_000);
  if (port) return port;
  killProfile(profile);
  if (process.platform !== "win32") {
    throw new Error(`${exe} started but never opened its debugging port in 20 seconds.`);
  }
  console.error(`${exe} started from Node never opened its debugging port. Trying again through PowerShell's Start-Process.`);
  const list = browserArgs(profile).map((arg) => `'${arg.replace(/'/g, "''")}'`).join(",");
  execFileSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", `Start-Process -FilePath '${exe.replace(/'/g, "''")}' -ArgumentList ${list}`], { stdio: "ignore" });
  port = await devToolsPort(profile, 20_000);
  if (port) return port;
  killProfile(profile);
  throw new Error(`${exe} never opened its debugging port, started either from Node or through Start-Process. Close other Edge windows started with --headless, or give Chrome with --browser.`);
}

/* The DevTools protocol */

async function openPage(port) {
  let target;
  for (let waited = 0; waited < 10_000 && !target; waited += 250) {
    try {
      target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((item) => item.type === "page");
    } catch {
      // Not answering yet.
    }
    if (!target) await sleep(250);
  }
  if (!target) throw new Error(`The browser's debugging port ${port} lists no page.`);
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((done, fail) => {
    socket.addEventListener("open", done, { once: true });
    socket.addEventListener("error", () => fail(new Error("Could not open the page's debugging socket.")), { once: true });
  });
  let seq = 0;
  const waits = new Map();
  const listeners = [];
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.id && waits.has(message.id)) {
      waits.get(message.id)(message);
      waits.delete(message.id);
      return;
    }
    for (const listener of listeners) listener(message);
  });
  const send = (method, params = {}) =>
    new Promise((done, fail) => {
      const id = ++seq;
      waits.set(id, (message) => (message.error ? fail(new Error(`${method}: ${message.error.message}`)) : done(message.result)));
      socket.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async (expression) => {
    const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(`In the page: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
    return result.result?.value;
  };
  return { send, evaluate, on: (listener) => listeners.push(listener), close: () => socket.close() };
}

/** Polls `expression` until it is truthy, for at most `ms`. Null when it never was. */
async function until(cdp, expression, ms, every = 250) {
  const started = Date.now();
  for (;;) {
    const value = await cdp.evaluate(expression);
    if (value) return value;
    if (Date.now() - started >= ms) return null;
    await sleep(every);
  }
}

/** The whole page, or only what `selector` covers. Toasts are dismissed first:
 * they are fixed to the corner, and would lie over the top of the shot. */
async function screenshot(cdp, file, selector) {
  await cdp.evaluate(`document.querySelectorAll("#toasts > *").forEach((toast) => toast.remove()), true`);
  const box = await cdp.evaluate(
    selector
      ? `(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return { x: Math.max(0, r.left + scrollX - 8), y: Math.max(0, r.top + scrollY - 8), width: r.width + 16, height: r.height + 16 }; })()`
      : "({ x: 0, y: 0, width: document.documentElement.clientWidth, height: document.documentElement.scrollHeight })",
  );
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: true, clip: { ...box, scale: 1 } });
  writeFileSync(file, Buffer.from(data, "base64"));
  return { file, width: Math.round(box.width), height: Math.round(box.height) };
}

/* What the page shows. These read the page's own markup. */

const LOG_TEXT = `(document.querySelector("pre.log")?.innerText ?? "")`;
const ROW = (key) => `document.querySelector(${JSON.stringify(`#row-${key}`)})`;

const SETTLED = (key) => `(() => {
  const row = ${ROW(key)};
  const button = row?.querySelector(".run button");
  return !!button && button.textContent === "Run" && !!row.querySelector(".steps .verdict");
})()`;

/** The row shows its resolution waiting for other people or another device. */
const WAITING = (key) => `[...${ROW(key)}.querySelectorAll(".steps > li")].some((li) => li.innerText.includes("awaiting-signatures"))`;

const OUTCOME = (key) => `(() => {
  const row = ${ROW(key)};
  const verdict = row.querySelector(".steps .verdict");
  return {
    // "null" is a verdict the page could not check, such as a memo whose
    // transaction the node would not show.
    pass: verdict ? (verdict.dataset.pass === "null" ? null : verdict.dataset.pass === "true") : null,
    unchecked: verdict ? verdict.dataset.pass === "null" : false,
    verdict: verdict ? verdict.innerText.replace(/\\s+/g, " ").trim() : null,
    expect: row.querySelector(".expect")?.innerText.trim() || null,
    steps: [...row.querySelectorAll(".steps > li")].map((li) => li.innerText.replace(/\\s+/g, " ").trim()),
  };
})()`;

function setOptions(key, set) {
  return `(() => {
    const row = ${ROW(key)};
    for (const [name, value] of Object.entries(${JSON.stringify(set ?? {})})) {
      const control = row.querySelector("[name=" + JSON.stringify(name) + "]");
      if (!control) throw new Error("Row ${key} has no option " + name);
      if (control.type === "checkbox") control.checked = value === true;
      else control.value = value;
      control.dispatchEvent(new Event("change", { bubbles: true }));
    }
    return true;
  })()`;
}

async function connectAndPick(cdp, account, ms) {
  // The relay's wallet is chosen and asked silently on load. A site the app
  // has not granted gets no accounts that way, so then Connect is pressed and
  // the person approves it in the app.
  const asked = await until(cdp, `${LOG_TEXT}.includes("standard:connect [") || ${LOG_TEXT}.includes("error  standard:connect")`, 30_000);
  if (!asked) throw new Error("The page never asked the relay's wallet to connect. Is the relay running on that port?");
  const accounts = () => cdp.evaluate(`[...document.querySelectorAll("#account option")].filter((o) => o.value).map((o) => ({ value: o.value, text: o.textContent }))`);
  if ((await accounts()).length === 0) {
    console.log("No account from a silent connect: pressing Connect. Approve the connection in Airlock.");
    await cdp.evaluate(`document.querySelector("button.primary").click(), true`);
    const got = await until(cdp, `document.querySelectorAll("#account option[value]:not([value=''])").length > 0 || ${LOG_TEXT}.includes("error  standard:connect")`, ms);
    if (!got || (await accounts()).length === 0) {
      const tail = (await cdp.evaluate(LOG_TEXT)).trim().split("\n").slice(-3).join(" | ");
      throw new Error(`The wallet connected no account in ${ms / 1000} s. Last log lines: ${tail}`);
    }
  }
  const list = await accounts();
  const index = /^\d+$/.test(account) ? Number(account) : list.findIndex((option) => option.text.startsWith(`${account} · `) || option.text === account);
  const chosen = list[index];
  if (!chosen) throw new Error(`No account ${account}. The wallet gave: ${list.map((option, i) => `${i}: ${option.text}`).join(", ")}.`);
  await cdp.evaluate(`(() => {
    const select = document.querySelector("#account");
    if (select.value !== ${JSON.stringify(chosen.value)}) {
      select.value = ${JSON.stringify(chosen.value)};
      select.dispatchEvent(new Event("change", { bubbles: true }));
    }
    return true;
  })()`);
  return { address: chosen.value, label: chosen.text.includes(" · ") ? chosen.text.split(" · ")[0] : null };
}

async function main() {
  let options;
  try {
    options = parseArgs(process.argv.slice(2));
  } catch (err) {
    console.error(`${err.message}\n\nUsage: node drive-page.mjs <outdir> (--relay <port> | --fake) [--only <ids>] [--rpc <url>] [--chain <id>] [--account <index|label>] [--timeout <seconds>] [--browser <path>] [--airdrop]`);
    process.exit(2);
  }
  if (!existsSync(join(DIST, "index.html"))) {
    console.error("dist/ has no page. Run npm run build first.");
    process.exit(2);
  }
  const outdir = resolve(options.outdir);
  mkdirSync(join(outdir, "rows"), { recursive: true });
  const runs = selectRuns(options.only);
  const timeoutMs = options.timeout * 1000;
  const cleanups = [];
  const cleanup = async () => {
    for (const step of cleanups.reverse()) {
      try {
        await step();
      } catch {
        // Go on stopping the rest.
      }
    }
  };
  const fail = async (message) => {
    console.error(message);
    await cleanup();
    process.exit(2);
  };
  process.on("SIGINT", () => void fail("Stopped."));

  // The relay, and for --fake the app and node behind it.
  let relayPort = options.relay;
  let rpc = options.rpc ?? "http://127.0.0.1:8899";
  if (options.fake) {
    const rpcPort = await freePort();
    const relayLog = [];
    const relay = await startRelay({
      port: 0,
      hostPath: join(HERE, "test", "fake-app.mjs"),
      env: { ...process.env, FAKE_RPC_PORT: String(rpcPort), FAKE_CHAIN: options.chain },
      log: (direction, origin, message) => relayLog.push(logLine(direction, origin, message)),
    });
    cleanups.push(() => relay.close());
    cleanups.push(() => writeFileSync(join(outdir, "relay.log"), relayLog.join("\n") + "\n"));
    relayPort = relay.port;
    if (!options.rpc) rpc = `http://127.0.0.1:${rpcPort}`;
    // The fake node lives in the fake app, so it is started before the page
    // first asks the node anything.
    relay.relay.start();
    let up = false;
    for (let waited = 0; waited < 15_000 && !up; waited += 200) {
      up = await fetch(`http://127.0.0.1:${rpcPort}`, { method: "POST", body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getSlot" }) }).then(
        () => true,
        () => false,
      );
      if (!up) await sleep(200);
    }
    if (!up) return fail("The fake app's node never answered.");
  }

  const server = await serve(0);
  cleanups.push(() => new Promise((done) => server.close(() => done())));
  const pageUrl = `http://127.0.0.1:${server.address().port}/?wallet=${encodeURIComponent(`ws://127.0.0.1:${relayPort}`)}`;

  let exe;
  try {
    exe = findBrowser(options.browser);
  } catch (err) {
    return fail(err.message);
  }
  // A new profile each run: a killed Edge can hold the last one.
  const profile = mkdtempSync(join(tmpdir(), "intents-drive-"));
  cleanups.push(async () => {
    killProfile(profile);
    for (let attempt = 0; attempt < 10; attempt += 1) {
      try {
        rmSync(profile, { recursive: true, force: true });
        return;
      } catch {
        await sleep(500);
      }
    }
  });
  let cdp;
  try {
    const port = await launchBrowser(exe, profile);
    cdp = await openPage(port);
  } catch (err) {
    return fail(err.message);
  }
  cleanups.push(async () => {
    await cdp.send("Browser.close").catch(() => {});
    cdp.close();
  });

  const problems = [];
  cdp.on((message) => {
    if (message.method === "Runtime.exceptionThrown") problems.push(`exception: ${message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text}`);
    if (message.method === "Runtime.consoleAPICalled" && message.params.type === "error") problems.push(`console.error: ${message.params.args.map((arg) => arg.value ?? arg.description).join(" ")}`);
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") problems.push(`log: ${message.params.entry.text} ${message.params.entry.url ?? ""}`.trim());
    if (message.method === "Network.requestWillBeSent") {
      const url = new URL(message.params.request.url);
      if ((url.protocol === "http:" || url.protocol === "https:" || url.protocol === "ws:") && !["127.0.0.1", "localhost", "[::1]"].includes(url.hostname)) {
        problems.push(`request off this machine: ${message.params.request.url}`);
      }
    }
  });
  for (const domain of ["Runtime", "Log", "Page", "Network"]) await cdp.send(`${domain}.enable`);
  // The network is set before the page's first script, so it never reads devnet first.
  const network = { kind: "custom", url: rpc, chain: options.chain };
  await cdp.send("Page.addScriptToEvaluateOnNewDocument", {
    source: `try { localStorage.setItem("airlock-intents.network", ${JSON.stringify(JSON.stringify(network))}); } catch {}`,
  });
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  await cdp.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
  await cdp.send("Page.navigate", { url: pageUrl });
  if (!(await until(cdp, `document.readyState === "complete" && !!document.querySelector("#wallet option")`, 30_000))) {
    return fail(`The page did not load from ${pageUrl}.`);
  }
  console.log(`Page ${pageUrl}, node ${rpc} as ${options.chain}${options.fake ? ", fake app" : `, relay on port ${relayPort}`}.`);

  let account;
  try {
    account = await connectAndPick(cdp, options.account, timeoutMs);
  } catch (err) {
    await screenshot(cdp, join(outdir, "page-light.png")).catch(() => {});
    return fail(err.message);
  }
  console.log(`Acting as ${account.label ? `${account.label} ` : ""}${account.address}.`);
  if (options.airdrop) {
    // The app key pays and signs in rows 5 and 6: on a local node it starts empty.
    const lines = `${LOG_TEXT}.split("\\n").filter((l) => l.includes("requestAirdrop"))`;
    const before = await cdp.evaluate(`${lines}.length`);
    await cdp.evaluate(`[...document.querySelectorAll("section.settings button")].filter((b) => b.textContent === "Airdrop 1 SOL").pop().click(), true`);
    // A call line, then a reply or an error.
    const done = await until(cdp, `(() => { const all = ${lines}; return all.length >= ${before} + 2 ? all[all.length - 1] : null; })()`, 60_000);
    if (!done || !/\breply\b/.test(done)) return fail(`The app key's airdrop did not go through: ${done ?? "no answer in 60 s"}`);
    console.log("Airdropped 1 SOL to the app key.");
  }
  // The page reads the profile on its own when the account is chosen.
  await until(cdp, SETTLED("profile"), timeoutMs);

  const results = [];
  for (const run of runs) {
    const started = Date.now();
    await cdp.evaluate(setOptions(run.row, run.set));
    const blocked = await cdp.evaluate(`${ROW(run.row)}.querySelector(".run button").disabled ? ${ROW(run.row)}.querySelector(".why").innerText || "disabled" : null`);
    if (blocked) {
      results.push({ id: run.id, row: run.row, options: run.set ?? {}, pass: null, verdict: null, blocked, timedOut: false, ms: 0, steps: [] });
      console.log(`${run.id.padEnd(20)} not run: ${blocked}`);
      continue;
    }
    await cdp.evaluate(`${ROW(run.row)}.querySelector(".run button").click(), true`);
    // A resolution that waits for other people or another device is shot as
    // it waits, the row and the table, and said on a line of its own
    // ("waiting <run>"), so whoever plays them acts after the shot.
    let settled = null;
    let waitingShot = null;
    for (const begun = Date.now(); !settled && Date.now() - begun < timeoutMs; ) {
      settled = await cdp.evaluate(SETTLED(run.row));
      if (settled) break;
      if (!waitingShot && (await cdp.evaluate(WAITING(run.row)))) {
        mkdirSync(join(outdir, "waiting"), { recursive: true });
        const name = run.id.replace(/:/g, "-");
        await screenshot(cdp, join(outdir, "waiting", `${name}-row.png`), `#row-${run.row}`);
        waitingShot = await screenshot(cdp, join(outdir, "waiting", `${name}-resolutions.png`), "section[aria-labelledby=resolutions-title]");
        console.log(`waiting ${run.id}`);
      }
      await sleep(250);
    }
    const outcome = await cdp.evaluate(OUTCOME(run.row));
    const shot = await screenshot(cdp, join(outdir, "rows", `${run.id.replace(/:/g, "-")}.png`), `#row-${run.row}`);
    const result = { id: run.id, row: run.row, options: run.set ?? {}, ...outcome, timedOut: !settled, ms: Date.now() - started, screenshot: shot.file, waitingScreenshot: waitingShot?.file ?? null };
    results.push(result);
    const word = !settled ? "TIMED OUT" : result.pass ? "pass" : result.unchecked ? "NOT CHECKED" : "NOT AS HOPED";
    console.log(`${run.id.padEnd(20)} ${word.padEnd(13)} ${result.verdict ?? ""}`);
  }

  const resolutions = await cdp.evaluate(`[...document.querySelectorAll("table.resolutions tbody tr")].map((tr) => [...tr.cells].map((td) => td.innerText.replace(/\\s+/g, " ").trim()))`);
  const log = (await cdp.evaluate(LOG_TEXT)).split("\n").filter(Boolean);
  const shots = [];
  shots.push(await screenshot(cdp, join(outdir, "settings.png"), "section.settings"));
  shots.push(await screenshot(cdp, join(outdir, "resolutions.png"), "section[aria-labelledby=resolutions-title]"));
  shots.push(await screenshot(cdp, join(outdir, "page-light.png")));
  await cdp.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
  shots.push(await screenshot(cdp, join(outdir, "page-dark.png")));
  const horizontalScroll = await cdp.evaluate("document.documentElement.scrollWidth > document.documentElement.clientWidth");

  const passed = results.filter((result) => result.pass === true).length;
  const outcome = {
    page: pageUrl,
    relay: relayPort,
    fake: options.fake,
    rpc,
    chain: options.chain,
    account,
    passed,
    failed: results.length - passed,
    runs: results,
    resolutions,
    log,
    problems,
    horizontalScroll,
    screenshots: shots,
  };
  writeFileSync(join(outdir, "outcome.json"), JSON.stringify(outcome, null, 2) + "\n");
  console.log(`${passed} of ${results.length} passed. ${problems.length ? `${problems.length} problems in the page. ` : ""}Wrote ${join(outdir, "outcome.json")} and the screenshots.`);
  await cleanup();
  process.exit(passed === results.length && problems.length === 0 ? 0 : 1);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
