// The intents test page against the real desktop app, end to end, from built
// binaries: a local validator (surfpool, offline, with the Squads and
// programmatic signer programs planted), the debug app on its own data folder
// with wallets funded on that node, the relay to its bridge-host, an approver
// that plays the person in the app's window, whoever else signs (another
// member of a multisig, another device), and the page driver pressing the
// rows. See "Driving the desktop app" in AGENTS.md and
// examples/intents/README.md.
//
//   cargo build -p airlock -p bridge-host
//   (cd examples/intents && npm install && npm run build)
//   node scripts/drive-intents-e2e.mjs <outdir> [--wallet <kind>] [--only <ids>] [--keep] [--hold <s>]
//
//   --wallet <kind>  the account the page acts as (default hot):
//                      hot             a hot key, every row
//                      multisig-alone  a Squads multisig of one member, a hot key
//                                      here: it resolves at once, as a carrier,
//                                      and a new mint lands at an ephemeral
//                                      signer of its proposal
//                      multisig-waits  a Squads multisig of two, both hot keys
//                                      here, the second not ticked to approve
//                                      with the first: requests wait for the
//                                      second member, who approves and executes
//                                      on the Proposals screen, a new mint
//                                      included; then the same with the second
//                                      ticked, and a proposal the second
//                                      rejects, and one cancelled
//                      offline         a key on another device, played by this
//                                      script: its signature is pasted into
//                                      Review and Sign's inbox
//                      programmatic    a programmatic signer whose authority is
//                                      a hot key here
//   --only <ids>     page runs to make, by id or row (as drive-page.mjs takes
//                    them), out of the ones the kind runs
//   --keep           leave the run's temp folder (app data, node ledger) behind
//   --hold <s>       keep the node, the app and the relay up that long after
//                    the page is done, to look at them
//
// Windows only, as the app driving is. It writes to <outdir>, for each phase
// of the kind (one folder each when it has more than one): outcome.json,
// page-*.png, settings.png, resolutions.png, rows/ and waiting/ from the page
// driver; modals/ with each request modal the approver acted on, named by the
// page run it belongs to, and the app's window at the moments another party
// acted (others/); approver.json; chain.json, every transaction each
// resolution made as the node returns it; summary.json, what was expected and
// what happened; and the logs of the node, the app, the relay and the page
// driver. It stops everything it started, also when it fails, and exits 0
// only when every run passed, the approver got past every modal, every check
// of the kind held and the node holds every transaction.
import { spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { copyFileSync, createWriteStream, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  PLANTED_PROGRAMS, actOnProposal, addOfflineWallet, addWallet, b58decode, b58encode, createMultisig,
  findProgramAddress, keyFromSeed, legacyTx, multisigSettings, openPage, plantProgram, plantSquadsConfig,
  root, rpcCall, selectWallet, setCustomRpc, sleep, startApp, startApprover, startNode, stopByFolder,
  stopTree,
} from "./drive-lib.mjs";
import { offlineSigner, programmaticSigner } from "./drive-others.mjs";
import { logLine, startRelay } from "../examples/intents/relay.mjs";
import { RUNS, selectRuns } from "../examples/intents/drive-page.mjs";

const args = process.argv.slice(2);
const takes = new Set(["--only", "--hold", "--wallet"]);
const outArg = args.find((a, i) => !a.startsWith("--") && !takes.has(args[i - 1]));
const option = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : null);
const only = option("--only");
const keep = args.includes("--keep");
const hold = Number(option("--hold") ?? 0);
const kind = option("--wallet") ?? "hot";
const KINDS = ["hot", "multisig-alone", "multisig-waits", "offline", "programmatic"];
if (!outArg || !KINDS.includes(kind)) {
  console.error(`usage: node scripts/drive-intents-e2e.mjs <outdir> [--wallet ${KINDS.join("|")}] [--only <ids>] [--keep] [--hold <s>]`);
  process.exit(2);
}
const out = resolve(outArg);
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const pageDist = join(root, "examples", "intents", "dist", "index.html");
if (!existsSync(pageDist)) {
  console.error("examples/intents/dist has no page: npm install && npm run build there first.");
  process.exit(2);
}

const started = Date.now();
const say = (text) => console.log(`[${((Date.now() - started) / 1000).toFixed(1).padStart(6)} s] ${text}`);
const freePort = () => new Promise((done, fail) => {
  const server = createServer();
  server.once("error", fail);
  server.listen(0, "127.0.0.1", () => {
    const { port } = server.address();
    server.close(() => done(port));
  });
});

// Everything of this run lives under one temp folder, so whatever outlives
// a kill (WebView2, headless Edge, surfpool) is found by its command line.
const work = mkdtempSync(join(tmpdir(), "intents-e2e-"));
const cleanups = [];
let cleaned = false;
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
  stopByFolder(work);
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
  console.error(`FAILED: ${message}`);
  await cleanup();
  process.exit(code);
};
process.on("SIGINT", () => void fail("stopped"));
process.on("uncaughtException", (err) => void fail(err.stack ?? err.message));
process.on("unhandledRejection", (err) => void fail(err?.stack ?? String(err)));

// What the kind is expected to do, checked after the page is done. Each
// failed check is one line.
const checks = [];
const check = (ok, text) => {
  checks.push({ ok: !!ok, text });
  say(`${ok ? "check ok" : "CHECK FAILED"}: ${text}`);
};

// ---- the node ----

// Alice and Bob are hot keys of the app. Olivia's key is on "another
// device": this script holds it, and the app has only her address.
const keys = { alice: keyFromSeed(randomBytes(32)), bob: keyFromSeed(randomBytes(32)), olivia: keyFromSeed(randomBytes(32)) };
const addressOf = (key) => b58encode(key.publicKey);
const [rpcPort, wsPort, debugPort] = [await freePort(), await freePort(), await freePort()];
const nodeLog = createWriteStream(join(out, "surfpool.log"));
let rpc;
try {
  const started = await startNode({ dir: join(work, "node"), rpcPort, wsPort, log: nodeLog, fund: Object.values(keys).map(addressOf) });
  cleanups.push(() => stopTree(started.node));
  started.node.on("error", (err) => void fail(`surfpool: ${err.message}`));
  rpc = started.url;
} catch (err) {
  await fail(err.message);
}
// The offline node has System, Memo, Token, Token-2022 and the associated
// token program. Squads v4 and the programmatic signer's three programs are
// planted from the binaries the flow tests run.
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

// ---- the app ----

const { env, app } = startApp(join(work, "app"), debugPort);
// Run after the app is stopped: cleanups run last first.
cleanups.push(() => existsSync(join(work, "app", "app.log")) && copyFileSync(join(work, "app", "app.log"), join(out, "app.log")));
cleanups.push(() => stopTree(app));
cleanups.push(() => stopByFolder(join(work, "app")));
const page = await openPage(debugPort).catch((err) => fail(`app: ${err.message}`));
cleanups.push(() => page.close());
const need = async (what, result) => {
  if (result !== "ok" && !(result && typeof result === "object" && !result.error)) await fail(`${what}: ${typeof result === "string" ? result : result?.error}`);
  return result;
};
await need("adding Alice", await addWallet(page, keys.alice, "Alice"));
await need("setting the custom RPC", await setCustomRpc(page, rpc));
say(`app up on debugging port ${debugPort}, Alice added, custom RPC in use`);

// ---- the kind's wallets ----

/** What the kind set up: the label and address the page acts as, the
 * profile the app should report, and its phases. */
const setup = {};
const othersDir = join(out, "others");
mkdirSync(othersDir, { recursive: true });
let otherShots = 0;
const shootApp = async (name) => {
  // A run id holds a colon (`order:serial`), which Windows reads as a stream of the file before it.
  const safe = name.replace(/[^A-Za-z0-9._-]+/g, "-");
  const file = join(othersDir, `${String(++otherShots).padStart(2, "0")}-${safe}.png`);
  await page.shotPage(file);
  return file;
};

if (kind === "hot") {
  Object.assign(setup, { label: "Alice", account: addressOf(keys.alice), profile: { timing: "sync", execution: "top-level" } });
} else if (kind === "multisig-alone" || kind === "multisig-waits") {
  const alone = kind === "multisig-alone";
  if (!alone) await need("adding Bob", await addWallet(page, keys.bob, "Bob"));
  const label = alone ? "Solo" : "Pair";
  const made = await need("creating the multisig", await createMultisig(page, {
    label, members: alone ? ["Alice"] : ["Alice", "Bob"], threshold: alone ? 1 : 2, payer: "Alice",
  }));
  // The page's rows spend from the account: a mint's rent, a transfer.
  await airdrop(made.vault, 10);
  await need("making the multisig current", await selectWallet(page, label));
  Object.assign(setup, { label, account: made.vault, multisig: made.multisig, profile: { timing: alone ? "sync" : "async", execution: "carrier" } });
  say(`multisig ${label} ${made.multisig}, vault ${made.vault}, ${alone ? "1 of 1 (Alice)" : "2 of 2 (Alice acts, Bob does not approve with her)"}`);
} else if (kind === "offline") {
  await need("adding Olivia", await addOfflineWallet(page, "Olivia", addressOf(keys.olivia)));
  await need("making Olivia current", await selectWallet(page, "Olivia"));
  Object.assign(setup, { label: "Olivia", account: addressOf(keys.olivia), profile: { timing: "async", execution: "top-level" } });
} else if (kind === "programmatic") {
  const made = await programmaticSigner.setup({ page, keys, rpc, say, need });
  await airdrop(made.address, 10);
  await need("making the signer current", await selectWallet(page, made.label));
  Object.assign(setup, { label: made.label, account: made.address, profile: { timing: "sync", execution: "carrier" } });
}

// ---- the relay ----

const relayLog = createWriteStream(join(out, "relay.log"));
// Every resolution the app pushed: the latest, every state it went through,
// and the page run it began in.
const pushed = new Map();
// Each profile the app answered, in order.
const profiles = [];
const listeners = [];
const relay = await startRelay({
  port: 0,
  hostPath: join(root, "target", "debug", "bridge-host.exe"),
  env,
  log: (direction, origin, message) => {
    relayLog.write(logLine(direction, origin, message) + "\n");
    if (direction !== "host>") return;
    if (message?.result?.profile) profiles.push({ run: current(), profile: message.result.profile });
    const resolution = message?.method === "resolutionChanged" ? message.params?.resolution : null;
    if (!resolution?.id) return;
    const before = pushed.get(resolution.id);
    const entry = { run: before?.run ?? current(), phase: before?.phase ?? phase?.name, resolution, states: [...(before?.states ?? [])] };
    if (entry.states.at(-1) !== resolution.state) entry.states.push(resolution.state);
    pushed.set(resolution.id, entry);
    for (const listener of listeners) listener(entry);
  },
  onStderr: (text) => relayLog.write(`host stderr: ${text}`),
});
cleanups.push(() => relay.close());
say(`relay on ws://127.0.0.1:${relay.port}`);

// ---- the phases ----

// The page run under way: the first of the phase's runs the page driver has
// not reported yet. "connect" before the first.
let phase = null;
const reported = new Set();
let connected = false;
const current = () => (!phase ? "setup" : connected ? phase.runs.find((run) => !reported.has(run.id))?.id ?? "after" : "connect");

/** The runs of `ids` that the kind makes, narrowed by --only. */
const runsOf = (ids) => {
  const chosen = selectRuns(ids.join(","));
  return only ? chosen.filter((run) => selectRuns(only).includes(run)) : chosen;
};

/** Every row but those the account cannot reach: the ones it refuses are
 * kept, since the refusal is what is checked. */
const ALL = RUNS.map((run) => run.id);

const phases = [];
if (kind === "hot") {
  phases.push({ name: "hot", runs: runsOf(ALL), approver: { "cosign:replaceable": { transaction: "review" }, return: { transaction: "review" } } });
} else if (kind === "multisig-alone") {
  phases.push({ name: "alone", runs: runsOf(ALL) });
} else if (kind === "multisig-waits") {
  // Every request that lands a proposal waits for Bob. The fallback is left
  // out: it would leave a proposal nobody follows.
  phases.push({
    name: "waits",
    runs: runsOf(ALL.filter((id) => id !== "fallback")),
    other: "approve",
  });
  phases.push({
    name: "together",
    runs: runsOf(["profile", "memo", "message:memo"]),
    before: async () => {
      await need("ticking Bob", await multisigSettings(page, setup.label, { together: ["Bob"] }));
      say("Bob ticked under Approves together with");
    },
    after: async () => {
      await need("unticking Bob", await multisigSettings(page, setup.label, { together: [] }));
    },
  });
  // The page's memo row hopes for done; here it should end failed, which
  // the checks below read from the resolution itself.
  phases.push({ name: "rejected", runs: runsOf(["memo"]), other: "reject", pageEnds: "failed" });
  phases.push({ name: "cancelled", runs: runsOf(["memo"]), other: "cancel", pageEnds: "failed" });
} else if (kind === "offline") {
  phases.push({ name: "offline", runs: runsOf(ALL.filter((id) => id !== "fallback")), approver: Object.fromEntries(ALL.map((id) => [id, { transaction: "review-only" }])), other: "offline" });
} else if (kind === "programmatic") {
  phases.push({ name: "programmatic", runs: runsOf(ALL) });
}

const approverLog = createWriteStream(join(out, "approver.log"));
const approver = startApprover({
  page,
  dir: join(out, "modals"),
  scenarios: new Proxy({}, { get: (_target, run) => phase?.approver?.[run] }),
  current,
  log: (line) => {
    approverLog.write(`${new Date().toISOString()} ${line}\n`);
    say(line);
  },
});
cleanups.push(() => approver.stop());

// ---- the other parties ----

/** Squads v4: an instruction of Anchor's `name`, its discriminator first. */
const anchor = (name) => createHash("sha256").update(`global:${name}`).digest().subarray(0, 8);
const SQUADS = b58decode("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");
const u64 = (n) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(n));
  return b;
};
const proposalOf = (multisig, index) => findProgramAddress([Buffer.from("multisig"), b58decode(multisig), Buffer.from("transaction"), u64(index), Buffer.from("proposal")], SQUADS)[0];
const transactionOf = (multisig, index) => findProgramAddress([Buffer.from("multisig"), b58decode(multisig), Buffer.from("transaction"), u64(index)], SQUADS)[0];
/** Ephemeral signer `signer` of transaction `index`: the address the program
 * signs for when it runs that transaction. */
const ephemeralOf = (multisig, index, signer) =>
  b58encode(findProgramAddress([Buffer.from("multisig"), transactionOf(multisig, index), Buffer.from("ephemeral_signer"), Buffer.from([signer])], SQUADS)[0]);
/** The multisig's newest transaction number, read from its account. */
const newestIndex = async (multisig) => {
  const info = await rpcCall(rpc, "getAccountInfo", [multisig, { encoding: "base64" }]);
  return Number(Buffer.from(info.value.data[0], "base64").readBigUInt64LE(8 + 32 + 32 + 2 + 4));
};
/** Bob votes on proposal `index` from another wallet, as a member does from
 * Squads' own app: `proposal_reject` or `proposal_cancel`, signed by his key,
 * sent straight to the node. */
const voteAs = async (key, vote, multisig, index) => {
  const { value } = await rpcCall(rpc, "getLatestBlockhash", [{ commitment: "confirmed" }]);
  const member = Buffer.from(key.publicKey);
  const wire = legacyTx({
    // ProposalVote: the multisig, the member (signs, pays), the proposal.
    keys: [member, proposalOf(multisig, index), b58decode(multisig), SQUADS],
    signers: 1, readonly: 2, blockhash: b58decode(value.blockhash),
    instructions: [[3, [2, 0, 1], Buffer.concat([anchor(vote), Buffer.from([0])])]],
  });
  const message = wire.subarray(65);
  const signed = Buffer.concat([wire.subarray(0, 1), key.sign(message), message]);
  const signature = await rpcCall(rpc, "sendTransaction", [signed.toString("base64"), { encoding: "base64" }]);
  for (let waited = 0; waited < 20_000; waited += 500) {
    const status = (await rpcCall(rpc, "getSignatureStatuses", [[signature]])).value[0];
    if (status?.confirmationStatus === "confirmed" || status?.confirmationStatus === "finalized") {
      if (status.err) throw new Error(`${vote} failed: ${JSON.stringify(status.err)}`);
      return signature;
    }
    await sleep(500);
  }
  throw new Error(`${vote} never confirmed`);
};

/** What the other parties did, in order. */
const othersDid = [];
// Resolutions another party has taken up, so each is played once.
const played = new Set();
/** Lines from the page driver: "waiting <run>" once it has shot a row that
 * waits. The other party acts after that, so the page is seen waiting. */
const waitingShot = new Set();

/** Play the other party for `entry`, a resolution that waits. */
const playOther = async (entry) => {
  const resolution = entry.resolution;
  const how = phase?.other;
  if (!how || played.has(resolution.id)) return;
  played.add(resolution.id);
  // Let the page shoot it waiting first.
  for (let waited = 0; waited < 20_000 && !waitingShot.has(entry.run); waited += 250) await sleep(250);
  approver.pause();
  try {
    if (how === "offline") {
      // One signature for each transaction of the request: a batch moves on
      // to its next one on Review and Sign once one is complete.
      const did = [];
      for (let round = 0; round < 16; round += 1) {
        did.push(await offlineSigner.sign({ page, key: keys.olivia, run: `${entry.run}-${round}`, shootApp, say }));
        await sleep(2500);
        if (pushed.get(resolution.id)?.resolution.state !== "awaiting-signatures") break;
      }
      othersDid.push({ run: entry.run, id: resolution.id, as: "Olivia's device", did: did.join("; ") });
      return;
    }
    const index = await newestIndex(setup.multisig);
    if (how === "approve") {
      await shootApp(`${entry.run}-waiting`);
      // One round for each proposal the request landed, oldest first:
      // the multisig runs them in order. Bob acts on the Proposals screen;
      // Alice is the acting member again before his transaction goes out,
      // since the page's next request follows at once and is rebuilt for
      // whoever acts then.
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
      const signature = await voteAs(keys.bob, "proposal_reject", setup.multisig, index);
      othersDid.push({ run: entry.run, id: resolution.id, as: "Bob, from another wallet", did: `rejected proposal ${index}`, signature });
    } else if (how === "cancel") {
      // Only an approved proposal can be cancelled: Bob approves, then both
      // cancel, as the threshold asks.
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
    approver.resume();
  }
};
listeners.push((entry) => {
  if (entry.resolution.state === "awaiting-signatures" && entry.phase === phase?.name) void playOther(entry);
});

// ---- the page, phase by phase ----

const driverLog = createWriteStream(join(out, "drive-page.log"));
const runIds = new Set(RUNS.map((run) => run.id));
const results = [];
for (const each of phases) {
  if (each.runs.length === 0) continue;
  phase = each;
  reported.clear();
  waitingShot.clear();
  connected = false;
  const dir = phases.length > 1 ? join(out, each.name) : out;
  mkdirSync(dir, { recursive: true });
  if (each.before) await each.before();
  say(`phase ${each.name}: ${each.runs.map((run) => run.id).join(", ")}`);
  const driverArgs = [
    join(root, "examples", "intents", "drive-page.mjs"), dir,
    "--relay", String(relay.port), "--rpc", rpc, "--chain", "solana:devnet", "--airdrop",
    "--timeout", "240", "--only", each.runs.map((run) => run.id).join(","), "--account", "0",
  ];
  // Its browser profile goes under this run's folder, so a leftover Edge is
  // found and stopped with the rest.
  const driverTemp = join(work, "browser");
  mkdirSync(driverTemp, { recursive: true });
  const driver = spawn(process.execPath, driverArgs, {
    env: { ...process.env, TEMP: driverTemp, TMP: driverTemp },
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  cleanups.push(() => stopTree(driver));
  let buffered = "";
  let actingAs = null;
  driver.stdout.on("data", (chunk) => {
    driverLog.write(chunk);
    buffered += chunk.toString("utf8");
    const lines = buffered.split(/\r?\n/);
    buffered = lines.pop();
    for (const line of lines) {
      if (line.startsWith("Acting as")) {
        connected = true;
        actingAs = line.match(/([1-9A-HJ-NP-Za-km-z]{32,44})\.?$/)?.[1] ?? null;
      }
      if (line.startsWith("waiting ")) waitingShot.add(line.split(/\s+/)[1]);
      const id = line.split(/\s+/)[0];
      if (runIds.has(id)) reported.add(id);
      say(`page: ${line}`);
    }
  });
  driver.stderr.on("data", (chunk) => {
    driverLog.write(chunk);
    say(`page stderr: ${chunk.toString("utf8").trim()}`);
  });
  const code = await new Promise((done) => driver.on("exit", (exit) => done(exit ?? 1)));
  results.push({ phase: each.name, code, dir });
  check(actingAs === setup.account, `phase ${each.name}: the page acted as ${setup.label} ${setup.account} (it acted as ${actingAs})`);
  if (each.pageEnds) {
    const outcome = JSON.parse(readFileSync(join(dir, "outcome.json"), "utf8"));
    for (const run of outcome.runs) {
      check(run.verdict?.includes(`It ended ${each.pageEnds}`), `phase ${each.name}: the page's ${run.id} row saw it end ${each.pageEnds} (${run.verdict})`);
    }
  } else {
    check(code === 0, `phase ${each.name}: every page run passed (drive-page exited ${code})`);
  }
  if (each.after) await each.after();
}
phase = null;

await approver.stop();
writeFileSync(join(out, "approver.json"), JSON.stringify({ actions: approver.actions, problems: approver.problems }, null, 2) + "\n");
say(`approver acted ${approver.actions.length} times, ${approver.problems.length} problems`);
for (const problem of approver.problems) console.error(`approver: ${problem}`);

// ---- what the node says ----

/** What a node shows of transaction `signature`: whether its status is
 * known and succeeded, and whether `getTransaction` returns it in any
 * encoding, with its logs. */
const readFromNode = async (signature) => {
  const read = { signature, status: null, err: null, fetched: null, encodings: {}, logs: [] };
  for (let attempt = 0; attempt < 20 && !read.status; attempt += 1) {
    const status = (await rpcCall(rpc, "getSignatureStatuses", [[signature], { searchTransactionHistory: true }])).value[0];
    if (status) {
      read.status = status.confirmationStatus ?? "processed";
      read.err = status.err ?? null;
    } else await sleep(500);
  }
  // Each encoding, asked for version 0 at most and then for version 1.
  for (const encoding of ["json", "base64", "jsonParsed"]) {
    for (const max of [0, 1]) {
      const key = `${encoding}, max ${max}`;
      try {
        const tx = await rpcCall(rpc, "getTransaction", [signature, { encoding, commitment: "confirmed", maxSupportedTransactionVersion: max }]);
        read.encodings[key] = tx ? "returned" : "null";
        if (tx && !read.fetched) {
          read.fetched = key;
          read.slot = tx.slot;
          read.version = tx.version ?? "legacy";
          read.logs = tx.meta?.logMessages ?? [];
        }
      } catch (err) {
        read.encodings[key] = err.message.replace(/^getTransaction: /, "");
      }
    }
  }
  return read;
};

// Every resolution the app pushed: each transaction it made and each it
// says ran, read back from the node.
const chain = [];
for (const { run, phase: phaseName, resolution, states } of pushed.values()) {
  const entry = { phase: phaseName, run, id: resolution.id, states, state: resolution.state, timing: resolution.timing, delivery: resolution.delivery, replaced: resolution.replaced ?? {}, transactions: [], ran: [] };
  chain.push(entry);
  for (const made of resolution.transactions ?? []) {
    if (!made.signature) continue;
    entry.transactions.push({ role: made.role, state: made.state, ...(await readFromNode(made.signature)) });
  }
  for (const ran of resolution.ran ?? []) {
    const read = await readFromNode(ran.signature);
    const memo = resolution.message?.memo;
    read.memoInLogs = memo === undefined ? undefined : read.logs.some((line) => line.includes(JSON.stringify(memo).slice(1, -1).slice(0, 60)));
    entry.ran.push({ transaction: ran.transaction, ...read });
    say(`node: ${phaseName} ${run} ${resolution.id} ran ${ran.transaction} in ${ran.signature.slice(0, 12)}… ${read.status ? `${read.status}, ${read.err === null ? "succeeded" : `FAILED ${JSON.stringify(read.err)}`}` : "NOT FOUND"}${read.fetched ? `, getTransaction (${read.fetched}) v${read.version}` : ", getTransaction refused"}${memo === undefined ? "" : read.memoInLogs ? ", memo in its logs" : ""}`);
  }
}
writeFileSync(join(out, "chain.json"), JSON.stringify({ rpc, account: setup.account, resolutions: chain }, null, 2) + "\n");
for (const entry of chain) {
  if (entry.state !== "done") continue;
  for (const ran of entry.ran) check(ran.status && ran.err === null, `${entry.run} ${entry.id}: what ran (${ran.signature.slice(0, 12)}…) is on the node and succeeded`);
  for (const made of entry.transactions) check(made.status && (made.err === null || made.role === "vote"), `${entry.run} ${entry.id}: its ${made.role} transaction is on the node and succeeded`);
}

// ---- what the kind expects ----

const firstProfile = profiles[0]?.profile;
if (setup.profile) {
  check(firstProfile?.timing === setup.profile.timing && firstProfile?.execution === setup.profile.execution,
    `the profile is ${setup.profile.timing}, ${setup.profile.execution} (the app said ${firstProfile ? `${firstProfile.timing}, ${firstProfile.execution}` : "nothing"})`);
}
const ofRun = (phaseName, run) => chain.filter((entry) => entry.phase === phaseName && entry.run === run);
const TOKEN_PROGRAMS = ["TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA", "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"];
/** The new mint of a resolution: the address `replaced` gave it, which has to
 * be ephemeral signer 0 of one of the multisig's proposals, and whether the
 * node holds an account there owned by a token program. */
const carriedMint = async (entry) => {
  const newest = await newestIndex(setup.multisig);
  for (const [sent, used] of Object.entries(entry.replaced)) {
    for (let index = 1; index <= newest; index += 1) {
      if (ephemeralOf(setup.multisig, index, 0) !== used) continue;
      const info = await rpcCall(rpc, "getAccountInfo", [used, { encoding: "base64" }]);
      return { sent, used, proposal: index, owner: info.value?.owner ?? null };
    }
  }
  return null;
};
const checkCarriedMint = async (entry, run) => {
  const mint = await carriedMint(entry);
  check(mint, `${run}: replaced gives the new mint an ephemeral signer of the multisig's proposal (${JSON.stringify(entry.replaced)})`);
  if (!mint) return;
  say(`node: ${run} mint ${mint.sent} became ${mint.used}, ephemeral signer 0 of proposal ${mint.proposal}, owned by ${mint.owner}`);
  check(TOKEN_PROGRAMS.includes(mint.owner), `${run}: the node holds the mint at ${mint.used}, owned by a token program (${mint.owner ?? "no account"})`);
};
if (kind === "multisig-alone") {
  for (const run of ["mint:classic", "mint:token-2022"]) {
    for (const entry of ofRun("alone", run)) {
      check(entry.state === "done" && !entry.states.includes("awaiting-signatures"), `${run}: done without waiting (${entry.states.join(" → ")})`);
      await checkCarriedMint(entry, run);
    }
  }
  for (const run of ["memo", "mint:classic", "message:memo"]) {
    for (const entry of ofRun("alone", run)) {
      check(entry.state === "done" && !entry.states.includes("awaiting-signatures"), `${run}: done without waiting (${entry.states.join(" → ")})`);
      check(entry.transactions.some((made) => made.role === "carrier"), `${run}: one carrier transaction created, approved and executed it (${entry.transactions.map((made) => made.role).join(", ")})`);
      check(entry.ran[0] && entry.transactions.some((made) => made.signature === entry.ran[0].signature), `${run}: ran names that transaction`);
    }
  }
}
if (kind === "multisig-waits") {
  for (const run of ["memo", "message:memo", "transfer"]) {
    for (const entry of ofRun("waits", run)) {
      check(entry.states.includes("awaiting-signatures") && entry.state === "done", `${run}: waited, then done once Bob executed (${entry.states.join(" → ")})`);
      check(entry.transactions.some((made) => made.role === "proposal"), `${run}: its transaction is listed as the proposal`);
      check(entry.ran[0] && !entry.transactions.some((made) => made.signature === entry.ran[0].signature), `${run}: ran names the executing transaction, not the proposal`);
    }
  }
  for (const run of ["mint:classic", "mint:token-2022"]) {
    const entries = ofRun("waits", run);
    check(entries.length > 0 || (only && !selectRuns(only).some((each) => each.id === run)), `${run}: the multisig took the request (${entries.length} resolution)`);
    for (const entry of entries) {
      check(entry.states.includes("awaiting-signatures") && entry.state === "done", `${run}: waited, then done once Bob executed (${entry.states.join(" → ")})`);
      await checkCarriedMint(entry, run);
    }
  }
  for (const entry of ofRun("together", "memo")) {
    check(entry.state === "done" && !entry.states.includes("awaiting-signatures"), `together, memo: done at once (${entry.states.join(" → ")})`);
  }
  check(profiles.some((item) => item.profile.timing === "sync"), "with Bob ticked the profile turns sync");
  for (const [phaseName, words] of [["rejected", "rejected"], ["cancelled", "cancelled"]]) {
    const entries = ofRun(phaseName, "memo");
    check(entries.length === 1 && entries[0].state === "failed" && entries[0].states.includes("awaiting-signatures"), `${phaseName}: the resolution waited, then failed (${entries.map((entry) => entry.states.join(" → ")).join("; ")})`);
    const error = [...pushed.values()].find((item) => item.phase === phaseName)?.resolution.error;
    check(error?.message?.includes(words), `${phaseName}: its error says the proposal was ${words} (${error?.message ?? "no error"})`);
  }
}
if (kind === "offline") {
  for (const entry of ofRun("offline", "memo")) {
    check(entry.states.includes("awaiting-signatures") && entry.state === "done", `memo: waited for the other device, then done (${entry.states.join(" → ")})`);
  }
}
for (const did of othersDid) check(!did.error, `${did.run}: ${did.as ?? "the other party"} ${did.did ?? did.error}`);

const summary = { kind, rpc, account: setup.account, label: setup.label, profiles, phases: results, othersDid, checks };
writeFileSync(join(out, "summary.json"), JSON.stringify(summary, null, 2) + "\n");

if (hold > 0) {
  say(`holding ${hold} s before stopping: node ${rpc}, app debugging port ${debugPort}, relay port ${relay.port}`);
  await sleep(hold * 1000);
}
await cleanup();
const failed = checks.filter((item) => !item.ok);
const passed = failed.length === 0 && approver.problems.length === 0;
console.log(passed ? `PASS: ${kind}, ${checks.length} checks held. Results in ${out}` : `FAIL: ${kind}, ${failed.length} of ${checks.length} checks failed${approver.problems.length ? `, ${approver.problems.length} approver problems` : ""}. See ${join(out, "summary.json")} and the logs in ${out}`);
process.exit(passed ? 0 : 1);
