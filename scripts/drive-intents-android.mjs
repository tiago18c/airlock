// The intents test page against the real Android app, end to end, on an
// emulator: a local validator on this machine (surfpool, offline, with the
// Squads and programmatic signer programs planted), the debug app on a fresh
// data folder with wallets funded on that node, the page served from here and
// opened in the emulator's Chrome with Mobile Wallet Adapter, every row
// pressed with a touch, an approver that plays the person in Airlock, and
// whoever else signs. The Android twin of scripts/drive-intents-e2e.mjs. See
// "Driving the app on Android" in AGENTS.md and examples/intents/README.md.
//
//   dx build --android -p airlock-mobile --target x86_64-linux-android
//   (cd examples/intents && npm install && npm run build)
//   node scripts/drive-intents-android.mjs <outdir> [--wallet <kind>] [--only <ids>] [--keep]
//
//   --wallet <kind>   the account the page acts as, as for drive-intents-e2e.mjs:
//                     hot, multisig-alone, multisig-waits, offline, programmatic
//   --only <ids>      page runs to make, by id or row, out of the ones the kind runs
//   --keep            leave the node's ledger folder, and the emulator when this
//                     run started it
//   --serial <s>      the emulator (default emulator-5560); --avd <name> the AVD
//                     to start when it is not up (default A16)
//   --apk <path>      the APK to install (default the debug APK dx builds);
//                     --no-install keeps the one installed
//   --tap cdp|adb     how a row is pressed: a touch through Chrome's DevTools
//                     protocol (default), or `adb shell input tap`
//
// It never builds. It needs the debug APK for x86_64 and the page built, and
// `surfpool` on the PATH. Nothing of the person's own data is touched: the
// app's data on the emulator is cleared first, and Chrome's storage for the
// page's origin.
//
// It writes to <outdir>, for each phase of the kind (one folder each when it
// has more than one): outcome.json, rows/ (each row as it settled), waiting/
// (a row and the Resolutions table while a resolution waited), page.png,
// resolutions.png; modals/ (the screen at each request the approver acted
// on), others/ (Airlock as the other parties acted), approver.json,
// chain.json, summary.json, drive.log, surfpool.log and logcat.txt. It exits
// 0 only when every run passed, the approver got past every modal, every
// check of the kind held and the node holds every transaction.
import { randomBytes } from "node:crypto";
import { appendFileSync, createWriteStream, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  PLANTED_PROGRAMS, actOnProposal, addOfflineWallet, addWallet, b58decode, b58encode, createMultisig, findProgramAddress, keyFromSeed,
  multisigSettings, plantProgram, plantSquadsConfig, root, rpcCall, setCustomRpc, sleep, startApprover,
  startNode, stopTree,
} from "./drive-lib.mjs";
import {
  CHROME, PKG, adb, allowPhoneApps, asApp, bringAppForward, bringChromeForward, chromeShot, ensureDir,
  ensureEmulator, foreground, openAppPage, openChromePage, prepareDevice, pushPrivate, screencap,
  selectPhoneWallet, shell, startApp, tapElement, tapNode, uiNodes,
} from "./drive-android.mjs";
import { KINDS, checkKind, readChain, squads } from "./drive-intents-checks.mjs";
import { offlineSigner, programmaticSigner } from "./drive-others.mjs";
import { LOG_TEXT, OUTCOME, ROW, RUNS, SETTLED, WAITING, selectRuns, setOptions } from "../examples/intents/drive-page.mjs";
import { serve } from "../examples/intents/serve.mjs";

const args = process.argv.slice(2);
const takes = new Set(["--only", "--wallet", "--serial", "--avd", "--apk", "--tap"]);
const outArg = args.find((a, i) => !a.startsWith("--") && !takes.has(args[i - 1]));
const option = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : null);
const only = option("--only");
const keep = args.includes("--keep");
const kind = option("--wallet") ?? "hot";
const serial = option("--serial") ?? "emulator-5560";
const avd = option("--avd") ?? "A16";
const tapHow = option("--tap") ?? "cdp";
const apk = resolve(option("--apk") ?? join(root, "target", "dx", "airlock-mobile", "debug", "android", "app", "app", "build", "outputs", "apk", "debug", "app-debug.apk"));
if (!outArg || !KINDS.includes(kind) || !["cdp", "adb"].includes(tapHow)) {
  console.error(`usage: node scripts/drive-intents-android.mjs <outdir> [--wallet ${KINDS.join("|")}] [--only <ids>] [--keep] [--serial <s>] [--avd <name>] [--apk <path> | --no-install] [--tap cdp|adb]`);
  process.exit(2);
}
if (["Medium_Phone_API_37.0"].includes(avd) || !/^emulator-\d+$/.test(serial)) {
  console.error("Only an emulator of this machine's own test AVDs is driven: its data is cleared.");
  process.exit(2);
}
const out = resolve(outArg);
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
if (!existsSync(join(root, "examples", "intents", "dist", "index.html"))) {
  console.error("examples/intents/dist has no page: npm install && npm run build there first.");
  process.exit(2);
}
if (!args.includes("--no-install") && !existsSync(apk)) {
  console.error(`${apk} is missing: dx build --android -p airlock-mobile --target x86_64-linux-android`);
  process.exit(2);
}

const started = Date.now();
// Written as it goes: the last lines come just before the process exits.
const say = (text) => {
  const line = `[${((Date.now() - started) / 1000).toFixed(1).padStart(6)} s] ${text}`;
  console.log(line);
  appendFileSync(join(out, "drive.log"), line + "\n");
};
const freePort = (port = 0) => new Promise((done, fail) => {
  const server = createServer();
  server.once("error", fail);
  server.listen(port, "127.0.0.1", () => {
    const { port: taken } = server.address();
    server.close(() => done(taken));
  });
});

const work = mkdtempSync(join(tmpdir(), "intents-android-"));
const cleanups = [];
let cleaned = false;
let startedEmulator = false;
const cleanup = async () => {
  if (cleaned) return;
  cleaned = true;
  for (const step of cleanups.reverse()) {
    try {
      await step();
    } catch (err) {
      console.error(`cleanup: ${err.message}`);
    }
  }
  if (startedEmulator && !keep) adb(serial, "emu", "kill");
  if (!keep) {
    for (let attempt = 0; attempt < 10; attempt += 1) {
      try {
        rmSync(work, { recursive: true, force: true });
        break;
      } catch {
        await sleep(1000);
      }
    }
  }
};
const fail = async (message, code = 2) => {
  say(`FAILED: ${message}`);
  await cleanup();
  process.exit(code);
};
process.on("SIGINT", () => void fail("stopped"));
process.on("uncaughtException", (err) => void fail(err.stack ?? err.message));
process.on("unhandledRejection", (err) => void fail(err?.stack ?? String(err)));

const checks = [];
const check = (ok, text) => {
  checks.push({ ok: !!ok, text });
  say(`${ok ? "check ok" : "CHECK FAILED"}: ${text}`);
};
const need = async (what, result) => {
  if (result !== "ok" && !(result && typeof result === "object" && !result.error)) await fail(`${what}: ${typeof result === "string" ? result : result?.error ?? "no answer"}`);
  return result;
};

// ---- the emulator ----

try {
  startedEmulator = await ensureEmulator({ serial, avd, log: say, logFile: join(work, "emulator.log") });
} catch (err) {
  await fail(err.message);
}
prepareDevice(serial);
adb(serial, "logcat", "-c");
cleanups.push(() => writeFileSync(join(out, "logcat.txt"), adb(serial, "logcat", "-d", "-v", "time")));
say(`emulator ${serial} up${startedEmulator ? " (started by this run)" : ""}`);

// ---- the node ----

const keys = { alice: keyFromSeed(randomBytes(32)), bob: keyFromSeed(randomBytes(32)), olivia: keyFromSeed(randomBytes(32)) };
const addressOf = (key) => b58encode(key.publicKey);
const [rpcPort, wsPort, appPort, chromePort] = [await freePort(), await freePort(), await freePort(), await freePort()];
let rpc;
try {
  const node = await startNode({ dir: join(work, "node"), rpcPort, wsPort, log: createWriteStream(join(out, "surfpool.log")), fund: Object.values(keys).map(addressOf) });
  cleanups.push(() => stopTree(node.node));
  rpc = node.url;
} catch (err) {
  await fail(err.message);
}
for (const [address, file] of [...PLANTED_PROGRAMS.squads, ...PLANTED_PROGRAMS.programmatic]) {
  await plantProgram(rpc, address, file).catch((err) => fail(`planting ${file}: ${err.message}`));
}
await plantSquadsConfig(rpc, addressOf(keys.alice));
say(`node ${rpc} up, Squads v4 and the programmatic signer planted; Alice ${addressOf(keys.alice)}, Bob ${addressOf(keys.bob)}, Olivia (offline) ${addressOf(keys.olivia)}`);
const airdrop = async (address, sol) => {
  await rpcCall(rpc, "requestAirdrop", [address, sol * 1_000_000_000]);
  for (let waited = 0; waited < 15_000; waited += 500) {
    if ((await rpcCall(rpc, "getBalance", [address])).value > 0) return;
    await sleep(500);
  }
  throw new Error(`the airdrop to ${address} never arrived`);
};

// ---- the page, served from here; the phone's localhost is this machine ----

let pagePort = 8124;
const server = await serve(pagePort).catch(async () => {
  pagePort = await freePort();
  return serve(pagePort);
});
cleanups.push(() => new Promise((done) => server.close(() => done())));
for (const port of [rpcPort, wsPort, pagePort]) adb(serial, "reverse", `tcp:${port}`, `tcp:${port}`);
cleanups.push(() => {
  for (const port of [rpcPort, wsPort, pagePort]) adb(serial, "reverse", "--remove", `tcp:${port}`);
  for (const port of [appPort, chromePort]) adb(serial, "forward", "--remove", `tcp:${port}`);
});
const pageUrl = `http://localhost:${pagePort}/?wallet=mwa`;
say(`page served on 127.0.0.1:${pagePort}, the phone's localhost:${pagePort} through adb reverse, as is the node's port`);

// ---- the app ----

if (!args.includes("--no-install")) {
  const installed = adb(serial, "install", "-r", apk);
  if (!/Success/.test(installed)) await fail(`installing ${apk}: ${installed.slice(-300)}`);
  say(`installed ${apk}`);
}
cleanups.push(() => shell(serial, `am force-stop ${PKG}`));
/** A fresh app: data cleared, online without the first launch's question,
 * a vault with Alice, the node as its custom RPC, Apps on this phone on.
 * Answers the page, or what went wrong. */
const freshApp = async () => {
  shell(serial, `am force-stop ${PKG}`);
  shell(serial, `pm clear ${PKG}`);
  asApp(serial, "mkdir -p no_backup");
  pushPrivate(serial, "no_backup/mode", Buffer.from("online"));
  asApp(serial, "chmod -R go-rwx no_backup");
  const app = await startApp(serial, appPort).then(() => openAppPage(serial, appPort)).catch((err) => ({ error: err.message }));
  if (app.error) return app;
  for (const [what, step] of [
    ["adding Alice", () => addWallet(app, keys.alice, "Alice")],
    ["setting the custom RPC", () => setCustomRpc(app, rpc)],
    ["turning on Apps on this phone", () => allowPhoneApps(app)],
  ]) {
    const result = await step();
    if (result !== "ok") {
      app.close();
      return { error: `${what}: ${result ?? "no answer"}`, died: app.closed() || !shell(serial, `pidof ${PKG}`) };
    }
  }
  return app;
};
let page = await freshApp();
// The emulator's WebView can crash on the first launch after an install (a
// seccomp failure in its sandboxed renderer), and the app with it: once,
// everything is done again.
if (page.error && page.died) {
  say(`the app died while being set up (${page.error}): once more from a cleared app`);
  page = await freshApp();
}
if (page.error) await fail(page.error);
cleanups.push(() => page.close());
say(`app up, Alice added, custom RPC ${rpc} in use, Apps on this phone on`);

// ---- the kind's wallets ----

const setup = {};
const othersDir = ensureDir(join(out, "others"));
let otherShots = 0;
const shootApp = async (name) => {
  const file = join(othersDir, `${String(++otherShots).padStart(2, "0")}-${name.replace(/[^A-Za-z0-9._-]+/g, "-")}.png`);
  screencap(serial, file);
  return file;
};
/** The vault and multisig account of the multisig labelled `label`. */
const phoneMultisig = async (label) => {
  const vault = await page.run(`
    nav('Wallets').click();
    const card = await until(() => walletCard(${JSON.stringify(label)}));
    return card?.querySelector('.addr-full code')?.title ?? null;
  `);
  if (!vault) return {};
  const { SQUADS } = squads(rpc);
  const accounts = await rpcCall(rpc, "getProgramAccounts", [b58encode(SQUADS), { encoding: "base64", dataSlice: { offset: 0, length: 0 } }]);
  for (const { pubkey } of accounts) {
    const [derived] = findProgramAddress([Buffer.from("multisig"), b58decode(pubkey), Buffer.from("vault"), Buffer.from([0])], SQUADS);
    if (b58encode(derived) === vault) return { vault, multisig: pubkey };
  }
  return { vault };
};

if (kind === "hot") {
  Object.assign(setup, { label: "Alice", account: addressOf(keys.alice), profile: { timing: "sync", execution: "top-level" } });
} else if (kind === "multisig-alone" || kind === "multisig-waits") {
  const alone = kind === "multisig-alone";
  if (!alone) await need("adding Bob", await addWallet(page, keys.bob, "Bob"));
  const label = alone ? "Solo" : "Pair";
  const created = await createMultisig(page, {
    label, members: alone ? ["Alice"] : ["Alice", "Bob"], threshold: alone ? 1 : 2, payer: "Alice",
  });
  // A phone's Details shows addresses shortened, so createMultisig reads
  // none there: the vault is the full address in the title of the card's,
  // and the multisig account the Squads account on the node whose vault it is.
  const made = created.error ? created : { ...created, ...(await phoneMultisig(label)) };
  await need("creating the multisig", made.vault && made.multisig ? made : { error: made.error ?? `no addresses read for ${label}` });
  await airdrop(made.vault, 10);
  await need("making the multisig current", await selectPhoneWallet(page, label));
  Object.assign(setup, { label, account: made.vault, multisig: made.multisig, profile: { timing: alone ? "sync" : "async", execution: "carrier" } });
  say(`multisig ${label} ${made.multisig}, vault ${made.vault}, ${alone ? "1 of 1 (Alice)" : "2 of 2 (Alice acts, Bob does not approve with her)"}`);
} else if (kind === "offline") {
  await need("adding Olivia", await addOfflineWallet(page, "Olivia", addressOf(keys.olivia)));
  await need("making Olivia current", await selectPhoneWallet(page, "Olivia"));
  Object.assign(setup, { label: "Olivia", account: addressOf(keys.olivia), profile: { timing: "async", execution: "top-level" } });
} else if (kind === "programmatic") {
  const made = await programmaticSigner.setup({ page, keys, rpc, say, need });
  await airdrop(made.address, 10);
  await need("making the signer current", await selectPhoneWallet(page, made.label));
  Object.assign(setup, { label: made.label, account: made.address, profile: { timing: "sync", execution: "carrier" } });
}
await shootApp("setup-done");

// ---- the phases, as drive-intents-e2e.mjs has them ----

const runsOf = (ids) => {
  const chosen = selectRuns(ids.join(","));
  return only ? chosen.filter((run) => selectRuns(only).includes(run)) : chosen;
};
const ALL = RUNS.map((run) => run.id);
const phases = [];
if (kind === "hot") {
  phases.push({ name: "hot", runs: runsOf(ALL), approver: { "cosign:replaceable": { transaction: "review" }, return: { transaction: "review" } } });
} else if (kind === "multisig-alone") {
  phases.push({ name: "alone", runs: runsOf(ALL) });
} else if (kind === "multisig-waits") {
  phases.push({ name: "waits", runs: runsOf(ALL.filter((id) => id !== "fallback")), other: "approve" });
  phases.push({
    name: "together",
    runs: runsOf(["profile", "memo", "message:memo"]),
    before: async () => {
      bringAppForward(serial);
      await need("ticking Bob", await multisigSettings(page, setup.label, { together: ["Bob"] }));
      say("Bob ticked under Approves together with");
    },
    after: async () => {
      bringAppForward(serial);
      await need("unticking Bob", await multisigSettings(page, setup.label, { together: [] }));
    },
  });
  phases.push({ name: "rejected", runs: runsOf(["memo"]), other: "reject", pageEnds: "failed" });
  phases.push({ name: "cancelled", runs: runsOf(["memo"]), other: "cancel", pageEnds: "failed" });
} else if (kind === "offline") {
  phases.push({ name: "offline", runs: runsOf(ALL.filter((id) => id !== "fallback")), approver: Object.fromEntries(ALL.map((id) => [id, { transaction: "review-only" }])), other: "offline" });
} else if (kind === "programmatic") {
  phases.push({ name: "programmatic", runs: runsOf(ALL) });
}

// The page run under way, as the approver names its screenshots.
let phase = null;
let currentRun = "setup";
const approverLog = createWriteStream(join(out, "approver.log"));
const approver = startApprover({
  page,
  dir: join(out, "modals"),
  scenarios: new Proxy({}, { get: (_target, run) => phase?.approver?.[run] }),
  current: () => currentRun,
  log: (line) => {
    approverLog.write(`${new Date().toISOString()} ${line}\n`);
    say(line);
  },
});
cleanups.push(() => approver.stop());

// ---- the other parties ----

const { newestIndex, voteAs } = squads(rpc);
const othersDid = [];
/** Resolutions another party has taken up, and those it is done with. */
const played = new Set();
const playedDone = new Set();

/** Play the other party for `entry`, a resolution that waits. Airlock is
 * brought forward for whoever acts in it. */
const playOther = async (entry) => {
  const resolution = entry.resolution;
  const how = phase?.other;
  if (!how || played.has(resolution.id)) return;
  played.add(resolution.id);
  approver.pause();
  try {
    if (how === "offline") {
      bringAppForward(serial);
      await sleep(1500);
      const did = [];
      for (let round = 0; round < 16; round += 1) {
        did.push(await offlineSigner.sign({ page, key: keys.olivia, run: `${entry.run}-${round}`, shootApp, say }));
        await sleep(2500);
        // The next transaction of a batch waits on Review and Sign, Share offered again.
        const more = await page.run(`return !!(await until(() => [...document.querySelectorAll('.content button')].some((b) => visible(b) && !b.disabled && b.innerText.trim() === 'Share with another device'), 4000));`);
        if (!more) break;
      }
      othersDid.push({ run: entry.run, id: resolution.id, as: "Olivia's device", did: did.join("; ") });
    } else if (how === "approve") {
      bringAppForward(serial);
      await sleep(1500);
      const index = await newestIndex(setup.multisig);
      await shootApp(`${entry.run}-waiting`);
      const proposals = (resolution.transactions ?? []).filter((made) => made.role === "proposal").length || 1;
      for (let round = 0; round < proposals; round += 1) {
        const acting = await multisigSettings(page, setup.label, { actingAs: "Bob" });
        if (acting !== "ok") throw new Error(`acting as Bob: ${acting}`);
        const done = await actOnProposal(
          page, setup.label, "Approve and execute", (name) => shootApp(`${entry.run}-${round}-${name}`),
          () => multisigSettings(page, setup.label, { actingAs: "Alice" }),
        );
        if (done !== "ok") {
          await multisigSettings(page, setup.label, { actingAs: "Alice" });
          throw new Error(`Bob's Approve and execute: ${done}`);
        }
      }
      await shootApp(`${entry.run}-bob-executed`);
      othersDid.push({ run: entry.run, id: resolution.id, as: "Bob, on the Proposals screen", did: `approved and executed proposal ${index}` });
    } else if (how === "reject") {
      const index = await newestIndex(setup.multisig);
      const signature = await voteAs(keys.bob, "proposal_reject", setup.multisig, index);
      othersDid.push({ run: entry.run, id: resolution.id, as: "Bob, from another wallet", did: `rejected proposal ${index}`, signature });
    } else if (how === "cancel") {
      const index = await newestIndex(setup.multisig);
      const approved = await voteAs(keys.bob, "proposal_approve", setup.multisig, index);
      const first = await voteAs(keys.alice, "proposal_cancel", setup.multisig, index);
      const second = await voteAs(keys.bob, "proposal_cancel", setup.multisig, index);
      othersDid.push({ run: entry.run, id: resolution.id, as: "Alice and Bob, from another wallet", did: `approved, then cancelled proposal ${index}`, signatures: [approved, first, second] });
    }
    say(`other party: ${othersDid.at(-1).as} ${othersDid.at(-1).did}`);
  } catch (err) {
    othersDid.push({ run: entry.run, id: resolution.id, error: err.message });
    say(`OTHER PARTY FAILED: ${err.message}`);
  } finally {
    playedDone.add(resolution.id);
    approver.resume();
  }
};

// ---- Chrome ----

let chrome = null;
/** Chrome's own prompts over the page (a first-run notice, an ad privacy
 * notice): declined with uiautomator. Answers what it pressed. */
const dismissChromePrompts = () => {
  const pressed = [];
  for (let round = 0; round < 4; round += 1) {
    const nodes = uiNodes(serial).filter((n) => n.pkg === CHROME && n.bounds);
    const button = nodes.find((n) => /^(No thanks|No, thanks|Got it|Dismiss|Skip|Not now|Use without an account)$/i.test(n.text));
    if (!button) break;
    tapNode(serial, button);
    pressed.push(button.text);
    shell(serial, "sleep 1");
  }
  if (pressed.length) say(`Chrome's prompts: pressed ${pressed.join(", ")}`);
  return pressed;
};
const chromeInFront = async () => {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    if (foreground(serial) === CHROME) return true;
    bringChromeForward(serial);
    await sleep(1200);
  }
  return foreground(serial) === CHROME;
};
/** Tap what `expression` (in the page) marks: it returns an element, which
 * gets `data-drive-tap`. Chrome is brought forward first. */
const tap = async (expression, what) => {
  await chromeInFront();
  const marked = await chrome.evaluate(`(() => {
    document.querySelectorAll('[data-drive-tap]').forEach((el) => el.removeAttribute('data-drive-tap'));
    const el = (${expression});
    if (!el) return false;
    el.setAttribute('data-drive-tap', '1');
    return true;
  })()`);
  if (!marked) return null;
  const at = await tapElement(serial, chrome, "[data-drive-tap]", tapHow);
  say(`tap (${tapHow}) ${what} at ${Math.round(at.x)},${Math.round(at.y)}`);
  return at;
};
const pageLog = async () => (await chrome.evaluate(LOG_TEXT, 8000).catch(() => null)) ?? "";
/** Every resolution the page keeps, by id. */
const pageStore = async () => {
  const raw = await chrome.evaluate(`localStorage.getItem("airlock-intents.resolutions")`, 5000).catch(() => null);
  try {
    return new Map((JSON.parse(raw ?? "[]") ?? []).map((item) => [item.id, item]));
  } catch {
    return new Map();
  }
};
const sessionOpen = async () => !!(await chrome.evaluate(`/session \\d+ open/.test(document.querySelector("#wallet")?.closest(".setting-body")?.innerText ?? "")`, 5000).catch(() => false));

// What every resolution went through, as the page saw it: the run and phase
// it began in, the latest, and each state.
const seen = new Map();
const note = (store, runId) => {
  for (const [id, item] of store) {
    if (!item.resolution) continue;
    const before = seen.get(id);
    if (!before && !runId) continue;
    const entry = { run: before?.run ?? runId, phase: before?.phase ?? phase?.name, resolution: item.resolution, states: [...(before?.states ?? [])] };
    if (entry.states.at(-1) !== item.resolution.state) entry.states.push(item.resolution.state);
    seen.set(id, entry);
  }
};

const ENDED = ["done", "failed", "unknown", "declined", "expired", "cancelled"];
const gestureFailures = [];

/** Press run `run`'s row and wait for it to settle, playing the other party
 * and asking with Check when a resolution moved on that the page cannot
 * hear of. */
const driveRun = async (run, dir) => {
  const started = Date.now();
  await chrome.evaluate(setOptions(run.row, run.set));
  const blocked = await chrome.evaluate(`${ROW(run.row)}.querySelector(".run button").disabled ? ${ROW(run.row)}.querySelector(".why").innerText || "disabled" : null`);
  if (blocked) {
    say(`${run.id.padEnd(20)} not run: ${blocked}`);
    return { id: run.id, row: run.row, options: run.set ?? {}, pass: null, verdict: null, blocked, timedOut: false, ms: 0, steps: [] };
  }
  const before = new Set((await pageStore()).keys());
  currentRun = run.id;
  await tap(`${ROW(run.row)}.querySelector(".run button")`, `${run.id}'s Run`);
  let settled = false;
  let waitingShot = null;
  let lastCheck = Date.now();
  const mine = new Set();
  let behindSince = null;
  while (!settled && Date.now() - started < 300_000) {
    await sleep(700);
    // Chrome's page answers nothing while it is behind. Airlock in front with
    // no request up for a while (after a Review flow it may stay): Chrome
    // comes forward again, as a person would switch back.
    if (foreground(serial) === CHROME) behindSince = null;
    else if (await page.evaluate(`!!document.querySelector('.modal.request')`, 3000)) behindSince = null;
    else if (behindSince === null) behindSince = Date.now();
    else if (Date.now() - behindSince > 12_000) {
      say("Chrome behind Airlock with no request up for 12 s: bringing Chrome forward");
      await chromeInFront();
      behindSince = null;
    }
    settled = !!(await chrome.evaluate(SETTLED(run.row), 5000).catch(() => false));
    const store = await pageStore();
    for (const id of store.keys()) if (!before.has(id)) mine.add(id);
    note(new Map([...store].filter(([id]) => mine.has(id))), run.id);
    if (settled) break;
    if (!waitingShot && (await chrome.evaluate(WAITING(run.row), 5000).catch(() => false))) {
      ensureDir(join(dir, "waiting"));
      const name = run.id.replace(/:/g, "-");
      await chromeInFront();
      await shootRegion(join(dir, "waiting", `${name}-row.png`), `#row-${run.row}`);
      await shootRegion(join(dir, "waiting", `${name}-resolutions.png`), "section[aria-labelledby=resolutions-title]");
      waitingShot = join(dir, "waiting", `${name}-row.png`);
      say(`waiting ${run.id}`);
    }
    // A resolution of this run waiting for others: they act, then Check asks.
    for (const id of mine) {
      const entry = seen.get(id);
      const state = entry?.resolution.state;
      if (!entry || ENDED.includes(state)) continue;
      if (state === "awaiting-signatures" && phase.other && !played.has(id)) {
        if (!waitingShot) continue; // shot first, as on desktop
        await playOther(entry);
        lastCheck = 0;
      }
      const othersPending = phase.other && state === "awaiting-signatures" && !playedDone.has(id);
      if (state === "awaiting-approval" || othersPending) continue;
      // Nobody tells the page: once no session is open, Check asks.
      if (Date.now() - lastCheck > 15_000 && !(await sessionOpen())) {
        lastCheck = Date.now();
        const at = await tap(`[...document.querySelectorAll("table.resolutions tbody tr")].find((tr) => tr.querySelector("code")?.title === ${JSON.stringify(id)})?.querySelector("button.sm:not(.ghost)")`, `Check for ${id}`);
        if (!at) say(`no Check button for ${id}`);
      }
    }
  }
  const outcome = await chrome.evaluate(OUTCOME(run.row));
  const rowsDir = ensureDir(join(dir, "rows"));
  await chromeInFront();
  const shot = join(rowsDir, `${run.id.replace(/:/g, "-")}.png`);
  await shootRegion(shot, `#row-${run.row}`);
  const result = { id: run.id, row: run.row, options: run.set ?? {}, ...outcome, resolutions: [...mine], timedOut: !settled, ms: Date.now() - started, screenshot: shot, waitingScreenshot: waitingShot };
  if (/needs a tap on this page/.test(outcome?.verdict ?? "")) gestureFailures.push(run.id);
  const word = !settled ? "TIMED OUT" : result.pass ? "pass" : result.unchecked ? "NOT CHECKED" : "NOT AS HOPED";
  say(`${run.id.padEnd(20)} ${word.padEnd(13)} ${result.verdict ?? ""}`);
  currentRun = "between";
  return result;
};

/** The part of the page `selector` covers, as Chrome draws it. */
const shootRegion = async (file, selector) => {
  const box = await chrome.evaluate(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) return null; const r = el.getBoundingClientRect(); return { x: Math.max(0, r.left + scrollX - 8), y: Math.max(0, r.top + scrollY - 8), width: r.width + 16, height: r.height + 16 }; })()`, 5000).catch(() => null);
  if (!box) return false;
  return chromeShot(chrome, file, box).catch(() => false);
};

/** A new page for the phase: Chrome's storage for the origin cleared, the
 * node set before the page's first script, then Connect pressed and
 * approved in Airlock. Answers the account the page acts as. */
const openPageForPhase = async () => {
  if (chrome) chrome.close();
  chrome = await openChromePage(serial, chromePort, pageUrl);
  await sleep(1500);
  dismissChromePrompts();
  const network = { kind: "custom", url: rpc, chain: "solana:devnet" };
  await chrome.evaluate(`localStorage.clear(); localStorage.setItem("airlock-intents.network", ${JSON.stringify(JSON.stringify(network))}); true`);
  await chrome.send("Page.reload");
  for (let i = 0; i < 60; i += 1) {
    await sleep(500);
    if (await chrome.evaluate(`document.readyState === "complete" && document.querySelector("#wallet")?.selectedOptions[0]?.text === "Mobile Wallet Adapter"`, 3000).catch(() => false)) break;
  }
  dismissChromePrompts();
  currentRun = "connect";
  await tap(`document.querySelector("section.settings button.primary")`, "Connect");
  let accounts = [];
  for (let waited = 0; waited < 120_000 && accounts.length === 0; waited += 1000) {
    await sleep(1000);
    accounts = (await chrome.evaluate(`[...document.querySelectorAll("#account option")].filter((o) => o.value).map((o) => o.value)`, 5000).catch(() => [])) ?? [];
  }
  if (accounts.length === 0) throw new Error(`the page connected no account. Log: ${(await pageLog()).split("\n").slice(-6).join(" | ")}`);
  const acting = await chrome.evaluate(`document.querySelector("#account").value`);
  say(`Acting as ${acting} (the wallet gave ${accounts.length} account${accounts.length === 1 ? "" : "s"})`);
  // The app key pays and signs in rows 5 and 6: on a local node it starts empty.
  await chrome.evaluate(`[...document.querySelectorAll("section.settings button")].filter((b) => b.textContent === "Airdrop 1 SOL").pop().click(), true`);
  for (let waited = 0; waited < 30_000; waited += 1000) {
    await sleep(1000);
    if (/reply\s+requestAirdrop/.test(await pageLog())) break;
  }
  // The profile is read on its own once the account is chosen, in the
  // session Connect leaves open a moment; the profile run reads it again.
  for (let waited = 0; waited < 20_000; waited += 1000) {
    if (await chrome.evaluate(SETTLED("profile"), 3000).catch(() => false)) break;
    await sleep(1000);
  }
  return acting;
};

// ---- the page, phase by phase ----

const results = [];
const profiles = [];
for (const each of phases) {
  if (each.runs.length === 0) continue;
  phase = each;
  const dir = phases.length > 1 ? ensureDir(join(out, each.name)) : out;
  if (each.before) await each.before();
  say(`phase ${each.name}: ${each.runs.map((run) => run.id).join(", ")}`);
  let acting = null;
  const runs = [];
  try {
    acting = await openPageForPhase();
    for (const run of each.runs) runs.push(await driveRun(run, dir));
  } catch (err) {
    say(`phase ${each.name} stopped: ${err.stack ?? err.message}`);
    runs.push({ id: "phase", pass: false, verdict: `stopped: ${err.message}` });
  }
  currentRun = "after";
  await chromeInFront();
  const log = (await pageLog()).split("\n").filter(Boolean);
  for (const line of log) {
    const match = line.match(/reply\s+getSigningProfile (\{.*\})\s*$/);
    if (match) {
      try {
        profiles.push({ phase: each.name, profile: JSON.parse(match[1]) });
      } catch {}
    }
  }
  note(await pageStore(), null);
  await shootRegion(join(dir, "resolutions.png"), "section[aria-labelledby=resolutions-title]");
  await shootRegion(join(dir, "settings.png"), "section.settings");
  await chromeShot(chrome, join(dir, "page.png")).catch(() => {});
  screencap(serial, join(dir, "screen.png"));
  const resolutionsTable = await chrome.evaluate(`[...document.querySelectorAll("table.resolutions tbody tr")].map((tr) => [...tr.cells].map((td) => td.innerText.replace(/\\s+/g, " ").trim()))`).catch(() => []);
  const passed = runs.filter((run) => run.pass === true).length;
  writeFileSync(join(dir, "outcome.json"), JSON.stringify({ page: pageUrl, tap: tapHow, rpc, chain: "solana:devnet", account: acting, passed, failed: runs.length - passed, runs, resolutions: resolutionsTable, log }, null, 2) + "\n");
  say(`phase ${each.name}: ${passed} of ${runs.length} runs passed`);
  results.push({ phase: each.name, passed, runs: runs.length, dir });
  check(acting === setup.account, `phase ${each.name}: the page acted as ${setup.label} ${setup.account} (it acted as ${acting})`);
  if (each.pageEnds) {
    for (const run of runs) check(run.verdict?.includes(`It ended ${each.pageEnds}`), `phase ${each.name}: the page's ${run.id} row saw it end ${each.pageEnds} (${run.verdict})`);
  } else {
    check(passed === runs.length, `phase ${each.name}: every page run passed (${passed} of ${runs.length}; not passed: ${runs.filter((run) => run.pass !== true).map((run) => run.id).join(", ") || "none"})`);
  }
  if (each.after) await each.after();
}
phase = null;
check(gestureFailures.length === 0, `no row was refused for want of a tap (${gestureFailures.join(", ") || "none"})`);

await approver.stop();
writeFileSync(join(out, "approver.json"), JSON.stringify({ actions: approver.actions, problems: approver.problems }, null, 2) + "\n");
say(`approver acted ${approver.actions.length} times, ${approver.problems.length} problems`);
for (const problem of approver.problems) say(`approver: ${problem}`);

// ---- what the node says, and what the kind expects ----

const chain = await readChain({ rpc, entries: seen.values(), say, check });
writeFileSync(join(out, "chain.json"), JSON.stringify({ rpc, account: setup.account, resolutions: chain }, null, 2) + "\n");
await checkKind({ kind, rpc, chain, profiles, setup, only, check, say });
for (const did of othersDid) check(!did.error, `${did.run}: ${did.as ?? "the other party"} ${did.did ?? did.error}`);

writeFileSync(join(out, "summary.json"), JSON.stringify({ kind, rpc, serial, tap: tapHow, account: setup.account, label: setup.label, profiles, phases: results, othersDid, checks }, null, 2) + "\n");
await cleanup();
const failed = checks.filter((item) => !item.ok);
const passed = failed.length === 0 && approver.problems.length === 0;
say(passed ? `PASS: ${kind}, ${checks.length} checks held. Results in ${out}` : `FAIL: ${kind}, ${failed.length} of ${checks.length} checks failed${approver.problems.length ? `, ${approver.problems.length} approver problems` : ""}. See ${join(out, "summary.json")} and the logs in ${out}`);
process.exit(passed ? 0 : 1);
