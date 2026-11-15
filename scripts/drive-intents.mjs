// Drives the desktop app through the intents prototype's bridge messages
// (docs/intents.md), without a browser or the extension, and screenshots the
// request modal for each. See "Driving the desktop app" in AGENTS.md.
//
//   cargo build -p airlock -p bridge-host
//   node scripts/drive-intents.mjs <outdir> [scenario...]
//
// Windows only. For a hot key it asks `getSigningProfile`, then sends each
// scenario of `scenarios` below: it screenshots the modal into
// <outdir>/<scenario>.png, prints the modal's text and buttons and every
// bridge reply and `resolutionChanged` push, and declines. With no scenario
// named, every one runs. The app gets its own vault, WebView2 profile and
// bridge pipe under <outdir>; DRIVE_PORT picks the debugging port (9556).
// With DRIVE_REVIEW=1 each request is opened on Review and Sign instead of
// declined in the modal, shot as <scenario>-review.png, and rejected there.
//
// Nothing here funds the key, so a simulation says the transaction would
// fail for want of SOL. The helpers are in scripts/drive-lib.mjs.
import { randomBytes } from "node:crypto";
import { join } from "node:path";
import {
  addWallet, b58decode, b58encode, connectSite, findProgramAddress, keyFromSeed,
  latestBlockhash, legacyTx, openPage, sleep, startApp, startHost,
} from "./drive-lib.mjs";

const out = process.argv[2];
if (!out) {
  console.error("usage: node scripts/drive-intents.mjs <outdir> [scenario...]");
  process.exit(2);
}
const port = Number(process.env.DRIVE_PORT ?? 9556);
const ORIGIN = "https://intents.example";
const CHAIN = "solana:devnet";

// ---- keys, programs ----

// Throwaway keys, the same every run so screenshots match. Never fund them.
// The mint's is new each run: a key the chain has seen fails the notes'
// check (`bad_notes`), and a fixed seed's key may have been used by anyone.
const wallet = keyFromSeed(Buffer.alloc(32, 1));
const siteMint = keyFromSeed(randomBytes(32));
const siteSigner = keyFromSeed(Buffer.alloc(32, 3));
const recipient = b58decode("4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T");
const system = Buffer.alloc(32);
const memoProgram = b58decode("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
const tokenProgram = b58decode("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const ataProgram = b58decode("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const blockhash = await latestBlockhash("devnet");
const b64 = (bytes) => Buffer.from(bytes).toString("base64");
const addr = (bytes) => b58encode(bytes);

const transferData = (lamports) => {
  const data = Buffer.alloc(12);
  data.writeUInt32LE(2, 0);
  data.writeBigUInt64LE(BigInt(lamports), 4);
  return data;
};

// ---- transactions ----

/** A memo the wallet signs: "Hello from a site". */
const memoTx = () => legacyTx({
  keys: [wallet.publicKey, memoProgram],
  readonly: 1,
  blockhash,
  instructions: [[1, [0], Buffer.from("Hello from a site")]],
});

/** A new mint whose key the site made, with the wallet's token account for
 * it, derived from the mint. */
const holder = findProgramAddress([wallet.publicKey, tokenProgram, siteMint.publicKey], ataProgram)[0];
const mintTx = () => {
  const create = Buffer.alloc(4 + 8 + 8 + 32);
  create.writeUInt32LE(0, 0);
  create.writeBigUInt64LE(1_461_600n, 4);
  create.writeBigUInt64LE(82n, 12);
  tokenProgram.copy(create, 20);
  const initialize = Buffer.concat([Buffer.from([20, 6]), wallet.publicKey, Buffer.from([0])]);
  // payer, mint | token account | System, Token, associated token
  return legacyTx({
    keys: [wallet.publicKey, siteMint.publicKey, holder, system, tokenProgram, ataProgram],
    signers: 2,
    readonly: 3,
    blockhash,
    instructions: [
      [3, [0, 1], create],
      [4, [1], initialize],
      [5, [0, 2, 0, 1, 3, 4], Buffer.from([1])],
    ],
  });
};
const mintNotes = {
  [addr(siteMint.publicKey)]: { kind: "new", replaceable: true },
  [addr(holder)]: {
    kind: "derived",
    pda: {
      program: addr(ataProgram),
      seeds: [{ account: addr(wallet.publicKey) }, { bytes: b64(tokenProgram) }, { account: addr(siteMint.publicKey) }],
    },
  },
};

/** A memo the wallet and the site's own key both sign. */
const cosignedTx = () => legacyTx({
  keys: [wallet.publicKey, siteSigner.publicKey, memoProgram],
  signers: 2,
  readonlySigned: 1,
  readonly: 1,
  blockhash,
  instructions: [[2, [0, 1], Buffer.from("Signed by the wallet and the site")]],
});

/** 5 lamports to a devnet address. */
const transferTx = () => legacyTx({
  keys: [wallet.publicKey, recipient, system],
  readonly: 1,
  blockhash,
  instructions: [[2, [0, 1], transferData(5)]],
});

// ---- scenarios: one bridge request each ----

const resolve = (id, transactions, rest = {}) => ({
  method: "resolve",
  params: { id, account: addr(wallet.publicKey), chain: CHAIN, transactions: transactions.map(b64), ...rest },
});
const scenarios = {
  memo: () => resolve("memo-1", [memoTx()]),
  mint: () => resolve("mint-1", [mintTx()], { accounts: mintNotes }),
  cosign: () => resolve("cosign-1", [cosignedTx()], {
    delivery: "cosign",
    accounts: { [addr(siteSigner.publicKey)]: { kind: "signer", signs: "app" } },
  }),
  return: () => resolve("return-1", [transferTx()], { delivery: "return" }),
  message: () => ({
    method: "resolveMessage",
    params: {
      id: "message-1",
      account: addr(wallet.publicKey),
      message: b64(Buffer.from("intents.example asks you to prove you hold this account.\nNonce: 7f3a91")),
      mode: "memo",
    },
  }),
};
const chosen = process.argv.slice(3);
// DRIVE_SIGN=cosign,return presses Sign for those scenarios instead of
// declining, and prints what the resolution became.
const signed = new Set((process.env.DRIVE_SIGN ?? "").split(",").filter(Boolean));
for (const name of chosen) {
  if (!scenarios[name]) {
    console.error(`unknown scenario ${name}; one of: ${Object.keys(scenarios).join(", ")}`);
    process.exit(2);
  }
}

// ---- run ----

const { env, stopApp } = startApp(out, port);
let host;
const stop = (code) => {
  host?.stopHost();
  stopApp();
  process.exit(code);
};
const page = await openPage(port).catch((err) => {
  console.error(err.message);
  stop(1);
});
const added = await addWallet(page, wallet);
if (added !== "ok") {
  console.error(`wallet: ${added}`);
  stop(1);
}
host = startHost(env);
host.frame({ v: 1, id: "status-1", method: "status" });
if ((await host.replyTo("status-1"))?.result?.role !== "desktop") {
  console.error("bridge-host does not reach the app");
  stop(1);
}
const accounts = await connectSite(page, host, ORIGIN);
if (!accounts.length) {
  console.error("the site did not connect");
  stop(1);
}

/** Print the frames that came back since `from`, one line each. */
const printFrames = (from) => {
  for (const f of host.since(from)) {
    if (f.method === "resolutionChanged") {
      const r = f.params?.resolution ?? {};
      const made = (r.transactions ?? []).map((t) => `${t.role}:${t.state}${t.wire ? ":wire" : ""}${t.signature ? ":signed" : ""}`).join(",");
      const owed = r.awaiting ? ` awaiting ${JSON.stringify(r.awaiting)}` : "";
      const replaced = Object.keys(r.replaced ?? {}).length ? ` replaced ${JSON.stringify(r.replaced)}` : "";
      console.log(`  push resolutionChanged ${f.origin}: ${r.id} ${r.state}${made ? ` [${made}]` : ""}${owed}${replaced}${r.error ? ` (${r.error.code}: ${r.error.message})` : ""}`);
    } else if (f.id) {
      console.log(`  reply ${f.id}: ${JSON.stringify(f.result ?? f.error)}`);
    }
  }
};

let seen = host.frames.length;
host.frame({ v: 1, id: "profile-1", origin: ORIGIN, method: "getSigningProfile", params: { account: addr(wallet.publicKey) } });
await host.replyTo("profile-1");
console.log("getSigningProfile:");
printFrames(seen);

let n = 0;
for (const name of chosen.length ? chosen : Object.keys(scenarios)) {
  const id = `${(++n).toString(16).padStart(4, "b")}-${name}`;
  const { method, params } = scenarios[name]();
  seen = host.frames.length;
  host.frame({ v: 1, id, origin: ORIGIN, method, expiresAt: Math.floor(Date.now() / 1000) + 600, params });
  await host.replyTo(id, 10000);
  // Ready once Review is enabled, or preparing failed, and no IDL is still
  // being looked up on chain.
  // A request whose notes fail a check leaves the queue, and the modal.
  const state = await page.run(`
    if (!await until(() => requestModal(), 10000)) return { error: 'no modal' };
    const gone = () => !requestModal();
    await until(() => { if (gone()) return true; const b = button('Review', requestModal()); return (b && !b.disabled) || requestModal().querySelector('.error'); }, 40000);
    await until(() => gone() || !document.body.innerText.includes('Looking for the IDLs'), 30000);
    await until(() => gone() || !document.body.innerText.includes('Simulating'), 20000);
    await w(1500);
    if (gone()) return { error: 'the modal closed by itself' };
    return { text: requestModal().innerText, buttons: modalButtons() };
  `);
  console.log(`\n=== ${name} (${method}) ===`);
  if (!state || state.error) {
    console.log(`  ${state?.error ?? "the page did not answer"}`);
    await sleep(1000);
    printFrames(seen);
    continue;
  }
  console.log(state.text.split("\n").filter((l) => l.trim()).map((l) => `  | ${l}`).join("\n"));
  console.log(`  buttons: ${state.buttons.join(", ")}`);
  await page.shotModal(join(out, `${name}.png`));
  if (signed.has(name)) {
    // Sign in the modal. A send needs a funded key; `cosign` and `return`
    // only hand the signed transaction to the resolution.
    const after = await page.run(`
      const b = await until(() => { const b = button('Sign', requestModal()) ?? button('Sign and send', requestModal()); return b && !b.disabled && b; }, 10000);
      if (!b) return 'Sign is not offered';
      b.click();
      await until(() => !requestModal(), 10000);
      await w(1500);
      return requestModal() ? requestModal().innerText.split('\\n').filter((l) => l.trim()).slice(-6).join(' / ') : 'answered';
    `);
    console.log(`  signed: ${after}`);
  } else if (process.env.DRIVE_REVIEW) {
    await page.run(`button('Review', requestModal())?.click(); await until(() => !requestModal()); await w(3000); return 1;`);
    await page.shotPage(join(out, `${name}-review.png`));
    await page.run(`button('Reject')?.click(); await w(800); return 1;`);
  } else {
    await page.run(`button('Reject', requestModal())?.click(); await w(800); return 1;`);
  }
  await sleep(1500);
  printFrames(seen);
}
stop(0);
